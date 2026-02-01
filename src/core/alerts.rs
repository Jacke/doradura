//! Alert management system for monitoring bot health and sending notifications
//!
//! This module implements an alert manager that monitors metrics and sends
//! Telegram notifications to the admin when critical issues are detected.
//!
//! Features:
//! - Multiple alert types (high error rate, queue backup, payment failures, etc.)
//! - Severity levels (Critical, Warning)
//! - Throttling to prevent alert spam
//! - Alert resolution notifications
//! - Database persistence of alert history

use crate::core::{config, metrics};
use crate::storage::db::{self, DbPool};
use crate::telegram::admin;
use crate::telegram::Bot;
use chrono::{DateTime, Duration, Utc};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration as StdDuration;
use teloxide::prelude::*;
use teloxide::types::{ChatId, ParseMode};
use tokio::sync::Mutex;

/// Check if PO Token server (bgutil) is available on port 4416
async fn check_po_token_server() -> bool {
    let client = match reqwest::Client::builder().timeout(StdDuration::from_secs(2)).build() {
        Ok(c) => c,
        Err(_) => return false,
    };

    match client.get("http://127.0.0.1:4416").send().await {
        Ok(resp) => resp.status().is_success() || resp.status().as_u16() == 404,
        Err(_) => false,
    }
}

/// Alert severity level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Warning level - requires attention but not urgent
    Warning,
    /// Critical level - requires immediate attention
    Critical,
}

impl Severity {
    /// Returns the emoji icon for this severity
    fn emoji(&self) -> &'static str {
        match self {
            Severity::Warning => "🟡",
            Severity::Critical => "🔴",
        }
    }
}

/// Type of alert
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AlertType {
    /// High error rate detected
    HighErrorRate,
    /// Queue backup (too many pending tasks)
    QueueBackup,
    /// Payment failure occurred
    PaymentFailure,
    /// yt-dlp health check failed
    YtdlpDown,
    /// Database connection issues
    DatabaseIssues,
    /// Low conversion rate
    LowConversion,
    /// High retry rate
    HighRetryRate,
    /// Cookies need refresh
    CookiesExpired,
    /// Download timeout rate high
    HighTimeoutRate,
    /// Disk space low
    LowDiskSpace,
    /// User complaint/negative feedback
    UserComplaint,
    /// System resource usage high
    HighResourceUsage,
}

impl AlertType {
    /// Returns the throttle window for this alert type (in seconds)
    fn throttle_window(&self) -> i64 {
        match self {
            AlertType::HighErrorRate => 1800,     // 30 minutes
            AlertType::QueueBackup => 900,        // 15 minutes
            AlertType::PaymentFailure => 0,       // No throttle - immediate
            AlertType::YtdlpDown => 300,          // 5 minutes
            AlertType::DatabaseIssues => 300,     // 5 minutes
            AlertType::LowConversion => 3600,     // 1 hour
            AlertType::HighRetryRate => 900,      // 15 minutes
            AlertType::CookiesExpired => 600,     // 10 minutes
            AlertType::HighTimeoutRate => 1800,   // 30 minutes
            AlertType::LowDiskSpace => 3600,      // 1 hour
            AlertType::UserComplaint => 0,        // No throttle - immediate
            AlertType::HighResourceUsage => 1800, // 30 minutes
        }
    }

    /// Returns a string identifier for this alert type
    fn as_str(&self) -> &'static str {
        match self {
            AlertType::HighErrorRate => "high_error_rate",
            AlertType::QueueBackup => "queue_backup",
            AlertType::PaymentFailure => "payment_failure",
            AlertType::YtdlpDown => "ytdlp_down",
            AlertType::DatabaseIssues => "database_issues",
            AlertType::LowConversion => "low_conversion",
            AlertType::HighRetryRate => "high_retry_rate",
            AlertType::CookiesExpired => "cookies_expired",
            AlertType::HighTimeoutRate => "high_timeout_rate",
            AlertType::LowDiskSpace => "low_disk_space",
            AlertType::UserComplaint => "user_complaint",
            AlertType::HighResourceUsage => "high_resource_usage",
        }
    }
}

/// Context information about a download attempt for better error reporting
#[derive(Debug, Clone, Default)]
pub struct DownloadContext {
    /// Which proxy was used (e.g., "WARP (Cloudflare)", "Residential (fallback)", "Direct")
    pub proxy_used: Option<String>,
    /// Whether cookies file was configured
    pub cookies_configured: bool,
    /// Whether PO Token server is available
    pub po_token_available: bool,
    /// Error type classification
    pub error_type: Option<String>,
    /// Player client used (e.g., "web,web_safari")
    pub player_client: Option<String>,
    /// Attempt number in proxy chain
    pub attempt_number: Option<u32>,
    /// Total proxies in chain
    pub total_proxies: Option<u32>,
}

impl DownloadContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create context with live status checks for cookies and PO Token
    pub async fn with_live_status() -> Self {
        let cookies_configured = config::YTDL_COOKIES_FILE.is_some();

        // Check PO Token server availability
        let po_token_available = check_po_token_server().await;

        // Get proxy info
        let proxy_used = config::proxy::WARP_PROXY
            .as_ref()
            .map(|_| "WARP (Cloudflare)".to_string());

        Self {
            cookies_configured,
            po_token_available,
            proxy_used,
            player_client: Some("web,web_safari".to_string()),
            ..Default::default()
        }
    }

    /// Format context as a human-readable string for alerts
    pub fn format_for_alert(&self) -> String {
        let mut lines = Vec::new();

        lines.push("📊 *Контекст скачивания:*".to_string());

        // Proxy info
        if let Some(ref proxy) = self.proxy_used {
            let attempt_info = match (self.attempt_number, self.total_proxies) {
                (Some(n), Some(t)) => format!(" (попытка {}/{})", n, t),
                _ => String::new(),
            };
            lines.push(format!("├ Прокси: {}{}", proxy, attempt_info));
        } else {
            lines.push("├ Прокси: не задан".to_string());
        }

        // Cookies status
        let cookies_status = if self.cookies_configured { "✅" } else { "❌" };
        lines.push(format!("├ Cookies: {}", cookies_status));

        // PO Token status
        let pot_status = if self.po_token_available { "✅" } else { "❌" };
        lines.push(format!("├ PO Token: {}", pot_status));

        // Player client
        if let Some(ref client) = self.player_client {
            lines.push(format!("├ Player: {}", client));
        }

        // Error type
        if let Some(ref err_type) = self.error_type {
            lines.push(format!("└ Тип ошибки: {}", err_type));
        } else {
            // Replace last ├ with └
            if let Some(last) = lines.last_mut() {
                *last = last.replace("├", "└");
            }
        }

        lines.join("\n")
    }
}

/// Alert message structure
#[derive(Debug, Clone)]
pub struct Alert {
    /// Type of alert
    pub alert_type: AlertType,
    /// Severity level
    pub severity: Severity,
    /// Alert title
    pub title: String,
    /// Alert message/description
    pub message: String,
    /// Additional details/breakdown
    pub details: Option<String>,
    /// Timestamp when alert was created
    pub triggered_at: DateTime<Utc>,
}

impl Alert {
    /// Creates a new alert
    pub fn new(
        alert_type: AlertType,
        severity: Severity,
        title: String,
        message: String,
        details: Option<String>,
    ) -> Self {
        Self {
            alert_type,
            severity,
            title,
            message,
            details,
            triggered_at: Utc::now(),
        }
    }

    /// Formats the alert as a Telegram message with MarkdownV2
    pub fn format_telegram_message(&self) -> String {
        let mut text = String::new();

        text.push_str(&format!(
            "{} *{} ALERT*\n\n",
            self.severity.emoji(),
            match self.severity {
                Severity::Critical => "CRITICAL",
                Severity::Warning => "WARNING",
            }
        ));

        text.push_str(&format!("⚠️ *{}*\n\n", admin::escape_markdown(&self.title)));

        text.push_str(&admin::escape_markdown(&self.message));
        text.push('\n');

        if let Some(details) = &self.details {
            text.push_str("\n\n*Details:*\n");
            text.push_str(&admin::escape_markdown(details));
        }

        text.push_str(&format!(
            "\n\n_Triggered: {}_",
            admin::escape_markdown(&self.triggered_at.format("%Y\\-%m\\-%d %H:%M:%S UTC").to_string())
        ));

        text
    }
}

/// Record of a download attempt (success or failure)
#[derive(Debug, Clone)]
struct DownloadRecord {
    timestamp: DateTime<Utc>,
    is_success: bool,
}

/// Alert manager for monitoring metrics and sending notifications
pub struct AlertManager {
    /// Telegram bot instance
    bot: Bot,
    /// Admin user ID to send alerts to
    admin_chat_id: ChatId,
    /// Database pool for persisting alert history
    db_pool: Arc<DbPool>,
    /// Last alert time for each alert type (for throttling)
    last_alert_time: Arc<Mutex<HashMap<AlertType, DateTime<Utc>>>>,
    /// Currently active alerts (for resolution detection)
    active_alerts: Arc<Mutex<HashMap<AlertType, Alert>>>,
    /// Recent download attempts for time-windowed error rate calculation
    recent_downloads: Arc<Mutex<VecDeque<DownloadRecord>>>,
    /// Last known counter values for delta calculation
    last_counter_values: Arc<Mutex<(f64, f64)>>, // (downloads, errors)
}

impl AlertManager {
    /// Creates a new AlertManager
    pub fn new(bot: Bot, admin_chat_id: ChatId, db_pool: Arc<DbPool>) -> Self {
        Self {
            bot,
            admin_chat_id,
            db_pool,
            last_alert_time: Arc::new(Mutex::new(HashMap::new())),
            active_alerts: Arc::new(Mutex::new(HashMap::new())),
            recent_downloads: Arc::new(Mutex::new(VecDeque::new())),
            last_counter_values: Arc::new(Mutex::new((0.0, 0.0))),
        }
    }

    /// Checks if an alert should be sent based on throttling rules
    async fn should_send_alert(&self, alert_type: &AlertType) -> bool {
        let last_times = self.last_alert_time.lock().await;

        if let Some(last_time) = last_times.get(alert_type) {
            let throttle_window = Duration::seconds(alert_type.throttle_window());
            let time_since_last = Utc::now() - *last_time;

            if time_since_last < throttle_window {
                log::debug!(
                    "Alert {:?} throttled (last sent {}s ago, window: {}s)",
                    alert_type,
                    time_since_last.num_seconds(),
                    throttle_window.num_seconds()
                );
                return false;
            }
        }

        true
    }

    /// Sends an alert to the admin
    pub async fn send_alert(&self, alert: Alert) -> Result<(), String> {
        // Check throttling
        if !self.should_send_alert(&alert.alert_type).await {
            return Ok(()); // Silently skip throttled alerts
        }

        log::warn!("Sending alert: {:?} - {}", alert.alert_type, alert.title);

        // Send Telegram message
        let message = alert.format_telegram_message();
        if let Err(e) = self
            .bot
            .send_message(self.admin_chat_id, &message)
            .parse_mode(ParseMode::MarkdownV2)
            .await
        {
            log::error!("Failed to send alert to admin: {:?}", e);
            return Err(format!("Failed to send alert: {:?}", e));
        }

        // Update last alert time
        {
            let mut last_times = self.last_alert_time.lock().await;
            last_times.insert(alert.alert_type.clone(), alert.triggered_at);
        }

        // Mark as active
        {
            let mut active = self.active_alerts.lock().await;
            active.insert(alert.alert_type.clone(), alert.clone());
        }

        // Save to database
        if let Ok(conn) = db::get_connection(&self.db_pool) {
            let severity_str = match alert.severity {
                Severity::Critical => "critical",
                Severity::Warning => "warning",
            };

            if let Err(e) = conn.execute(
                "INSERT INTO alert_history (alert_type, severity, message, triggered_at) VALUES (?, ?, ?, ?)",
                rusqlite::params![
                    alert.alert_type.as_str(),
                    severity_str,
                    format!("{}\n\n{}", alert.title, alert.message),
                    alert.triggered_at.to_rfc3339(),
                ],
            ) {
                log::error!("Failed to save alert to database: {}", e);
            }
        }

        Ok(())
    }

    /// Checks if an alert condition is resolved and sends resolution notification
    pub async fn check_resolution(&self, alert_type: &AlertType) -> Result<(), String> {
        let mut active = self.active_alerts.lock().await;

        if let Some(alert) = active.remove(alert_type) {
            log::info!("Alert {:?} resolved", alert_type);

            let message = format!(
                "✅ *Alert Resolved*\n\n{}\n\n_The issue has been resolved\\._",
                admin::escape_markdown(&alert.title)
            );

            if let Err(e) = self
                .bot
                .send_message(self.admin_chat_id, &message)
                .parse_mode(ParseMode::MarkdownV2)
                .await
            {
                log::error!("Failed to send resolution notification: {:?}", e);
                return Err(format!("Failed to send resolution: {:?}", e));
            }

            // Update database
            if let Ok(conn) = db::get_connection(&self.db_pool) {
                if let Err(e) = conn.execute(
                    "UPDATE alert_history SET resolved_at = ? WHERE alert_type = ? AND resolved_at IS NULL",
                    rusqlite::params![Utc::now().to_rfc3339(), alert_type.as_str()],
                ) {
                    log::error!("Failed to update alert resolution in database: {}", e);
                }
            }
        }

        Ok(())
    }

    /// Monitors metrics and triggers alerts based on thresholds
    pub async fn check_all_conditions(&self) -> Result<(), String> {
        // Only run checks if alerts are enabled
        if !*config::alerts::ENABLED {
            return Ok(());
        }

        self.check_error_rate().await?;
        self.check_queue_depth().await?;
        self.check_retry_rate().await?;

        Ok(())
    }

    /// Checks error rate and sends alert if threshold exceeded
    async fn check_error_rate(&self) -> Result<(), String> {
        use prometheus::core::Collector;

        // Get current counter values
        let mut current_downloads = 0.0;
        let mut current_errors = 0.0;

        // Sum all download successes
        for mf in metrics::DOWNLOAD_SUCCESS_TOTAL.collect() {
            for m in mf.get_metric() {
                current_downloads += m.get_counter().value();
            }
        }

        // Sum all download failures
        for mf in metrics::DOWNLOAD_FAILURE_TOTAL.collect() {
            for m in mf.get_metric() {
                current_errors += m.get_counter().value();
            }
        }

        // Update recent downloads buffer
        let mut recent = self.recent_downloads.lock().await;
        let now = Utc::now();
        let one_hour_ago = now - Duration::hours(1);

        // Remove downloads older than 1 hour
        while let Some(record) = recent.front() {
            if record.timestamp < one_hour_ago {
                recent.pop_front();
            } else {
                break;
            }
        }

        // Add new records (calculate delta from last check)
        let mut last_values = self.last_counter_values.lock().await;
        let (last_downloads, last_errors) = *last_values;

        let new_downloads = (current_downloads - last_downloads).max(0.0);
        let new_errors = (current_errors - last_errors).max(0.0);

        // Add success records
        for _ in 0..(new_downloads as u64) {
            recent.push_back(DownloadRecord {
                timestamp: now,
                is_success: true,
            });
        }

        // Add failure records
        for _ in 0..(new_errors as u64) {
            recent.push_back(DownloadRecord {
                timestamp: now,
                is_success: false,
            });
        }

        // Update last known values
        *last_values = (current_downloads, current_errors);
        drop(last_values);

        // Calculate error rate for last hour
        let total_recent = recent.len();
        if total_recent < 10 {
            // Not enough data yet
            drop(recent);
            return Ok(());
        }

        let errors_recent = recent.iter().filter(|r| !r.is_success).count();
        let error_rate = (errors_recent as f64 / total_recent as f64) * 100.0;
        let threshold = *config::alerts::ERROR_RATE_THRESHOLD;

        drop(recent); // Release lock

        if error_rate > threshold {
            let alert = Alert::new(
                AlertType::HighErrorRate,
                Severity::Critical,
                "High Error Rate Detected".to_string(),
                format!(
                    "Current (last hour): {:.1}% (threshold: {:.1}%)\nAffected: {}/{} downloads",
                    error_rate, threshold, errors_recent, total_recent
                ),
                Some("Recent performance issues detected. Check logs for details.".to_string()),
            );

            self.send_alert(alert).await?;
        } else {
            // Check if alert should be resolved
            self.check_resolution(&AlertType::HighErrorRate).await?;
        }

        Ok(())
    }

    /// Checks queue depth and sends alert if threshold exceeded
    async fn check_queue_depth(&self) -> Result<(), String> {
        use prometheus::core::Collector;

        let mut total_queue_depth = 0.0;

        for mf in metrics::QUEUE_DEPTH_TOTAL.collect() {
            for m in mf.get_metric() {
                total_queue_depth = m.get_gauge().value();
            }
        }

        let threshold = *config::alerts::QUEUE_DEPTH_THRESHOLD as f64;

        if total_queue_depth > threshold {
            let alert = Alert::new(
                AlertType::QueueBackup,
                Severity::Warning,
                "Queue Backup Detected".to_string(),
                format!(
                    "Current queue depth: {} tasks (threshold: {})",
                    total_queue_depth as u64, threshold as u64
                ),
                Some("Tasks are accumulating faster than they can be processed.".to_string()),
            );

            self.send_alert(alert).await?;
        } else {
            self.check_resolution(&AlertType::QueueBackup).await?;
        }

        Ok(())
    }

    /// Checks retry rate and sends alert if threshold exceeded
    async fn check_retry_rate(&self) -> Result<(), String> {
        use prometheus::core::Collector;

        let mut total_retries = 0.0;
        let mut total_tasks = 0.0;

        for mf in metrics::TASK_RETRIES_TOTAL.collect() {
            for m in mf.get_metric() {
                total_retries += m.get_counter().value();
            }
        }

        // Approximate total tasks from downloads
        for mf in metrics::DOWNLOAD_SUCCESS_TOTAL.collect() {
            for m in mf.get_metric() {
                total_tasks += m.get_counter().value();
            }
        }

        if total_tasks < 10.0 {
            return Ok(());
        }

        let retry_rate = (total_retries / total_tasks) * 100.0;
        let threshold = *config::alerts::RETRY_RATE_THRESHOLD;

        if retry_rate > threshold {
            let alert = Alert::new(
                AlertType::HighRetryRate,
                Severity::Warning,
                "High Retry Rate Detected".to_string(),
                format!(
                    "Current: {:.1}% (threshold: {:.1}%)\nRetries: {} of {} tasks",
                    retry_rate, threshold, total_retries as u64, total_tasks as u64
                ),
                Some("Tasks are frequently failing and being retried.".to_string()),
            );

            self.send_alert(alert).await?;
        } else {
            self.check_resolution(&AlertType::HighRetryRate).await?;
        }

        Ok(())
    }

    /// Sends a payment failure alert (called directly when payment fails)
    pub async fn alert_payment_failure(&self, plan: &str, reason: &str) -> Result<(), String> {
        let alert = Alert::new(
            AlertType::PaymentFailure,
            Severity::Critical,
            "Payment Failure".to_string(),
            format!("A {} subscription payment has failed", plan),
            Some(format!("Reason: {}\n\nPlease investigate immediately.", reason)),
        );

        self.send_alert(alert).await
    }

    /// Sends a cookies expired alert
    pub async fn alert_cookies_expired(&self, reason: &str) -> Result<(), String> {
        // Record metric
        metrics::record_alert("cookies_expired", "critical");

        let alert = Alert::new(
            AlertType::CookiesExpired,
            Severity::Critical,
            "Cookies Need Refresh".to_string(),
            "YouTube cookies have expired or become invalid.".to_string(),
            Some(format!(
                "Reason: {}\n\nDownloads will fail until cookies are refreshed.\nUse /update_cookies command to upload new cookies.",
                reason
            )),
        );

        self.send_alert(alert).await
    }

    /// Sends a download timeout alert
    pub async fn alert_high_timeout_rate(&self, timeout_rate: f64, threshold: f64) -> Result<(), String> {
        metrics::record_alert("high_timeout_rate", "warning");

        let alert = Alert::new(
            AlertType::HighTimeoutRate,
            Severity::Warning,
            "High Timeout Rate Detected".to_string(),
            format!(
                "Download timeout rate: {:.1}% (threshold: {:.1}%)",
                timeout_rate, threshold
            ),
            Some("Network issues or slow external services may be affecting downloads.".to_string()),
        );

        self.send_alert(alert).await
    }

    /// Sends a low disk space alert
    pub async fn alert_low_disk_space(&self, available_gb: f64, threshold_gb: f64) -> Result<(), String> {
        metrics::record_alert("low_disk_space", "critical");

        let alert = Alert::new(
            AlertType::LowDiskSpace,
            Severity::Critical,
            "Low Disk Space".to_string(),
            format!(
                "Available disk space: {:.1} GB (threshold: {:.1} GB)",
                available_gb, threshold_gb
            ),
            Some("Downloads may fail due to insufficient disk space. Please free up space.".to_string()),
        );

        self.send_alert(alert).await
    }

    /// Sends a user complaint alert
    pub async fn alert_user_complaint(
        &self,
        user_id: i64,
        username: Option<&str>,
        feedback: &str,
    ) -> Result<(), String> {
        metrics::record_alert("user_complaint", "warning");

        let user_display = username.map_or(format!("ID: {}", user_id), |u| format!("@{} ({})", u, user_id));

        let alert = Alert::new(
            AlertType::UserComplaint,
            Severity::Warning,
            "User Complaint Received".to_string(),
            format!("User {} reported an issue", user_display),
            Some(format!("Feedback:\n{}", feedback)),
        );

        self.send_alert(alert).await
    }

    /// Sends a download failure alert for a specific user
    ///
    /// Optionally includes download context (proxy used, cookies status, etc.)
    pub async fn alert_download_failure(
        &self,
        user_id: i64,
        url: &str,
        error: &str,
        retry_count: u32,
        context: Option<&DownloadContext>,
    ) -> Result<(), String> {
        // Only alert if retries exhausted
        if retry_count < 3 {
            return Ok(());
        }

        metrics::record_alert("download_failure", "warning");

        let truncated_url = if url.len() > 100 {
            format!("{}...", &url[..100])
        } else {
            url.to_string()
        };

        let truncated_error = if error.len() > 500 {
            format!("{}...", &error[..500])
        } else {
            error.to_string()
        };

        // Build details with context if available
        let details = if let Some(ctx) = context {
            format!(
                "{}\n\n🔴 *Ошибка:*\n```\n{}\n```",
                ctx.format_for_alert(),
                truncated_error
            )
        } else {
            format!("Error: {}", truncated_error)
        };

        let alert = Alert::new(
            AlertType::HighErrorRate,
            Severity::Warning,
            "Download Failure (Retries Exhausted)".to_string(),
            format!(
                "User {} failed to download after {} attempts:\n{}",
                user_id, retry_count, truncated_url
            ),
            Some(details),
        );

        self.send_alert(alert).await
    }
}

/// Starts the alert monitoring background task
///
/// This function spawns a background task that periodically checks metrics
/// and sends alerts to the admin when thresholds are exceeded.
pub async fn start_alert_monitor(bot: Bot, admin_chat_id: ChatId, db_pool: Arc<DbPool>) -> Arc<AlertManager> {
    let alert_manager = Arc::new(AlertManager::new(bot, admin_chat_id, db_pool));

    // Spawn background monitoring task
    let manager_clone = Arc::clone(&alert_manager);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));

        loop {
            interval.tick().await;

            if let Err(e) = manager_clone.check_all_conditions().await {
                log::error!("Alert monitoring error: {}", e);
            }
        }
    });

    log::info!("Alert monitoring started (checking every 60s)");

    alert_manager
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alert_format() {
        let alert = Alert::new(
            AlertType::HighErrorRate,
            Severity::Critical,
            "High Error Rate".to_string(),
            "Error rate: 10%".to_string(),
            Some("Check logs".to_string()),
        );

        let message = alert.format_telegram_message();
        assert!(message.contains("CRITICAL"));
        assert!(message.contains("High Error Rate"));
    }

    #[test]
    fn test_alert_severity_emoji() {
        assert_eq!(Severity::Critical.emoji(), "🔴");
        assert_eq!(Severity::Warning.emoji(), "🟡");
    }

    #[test]
    fn test_alert_type_throttle() {
        assert_eq!(AlertType::PaymentFailure.throttle_window(), 0);
        assert_eq!(AlertType::HighErrorRate.throttle_window(), 1800);
    }
}
