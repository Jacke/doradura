//! HTTP client for external Python cookie_manager.py.

use anyhow::Result;
use std::time::Duration;
use tokio::time::timeout;

use super::file_ops::log_cookie_file_diagnostics;

/// Cookie manager server endpoint (Python cookie_manager.py)
const COOKIE_MANAGER_BASE_URL: &str = "http://127.0.0.1:9876";

/// HTTP client for communicating with Python cookie_manager.py
///
/// Enables the feedback loop:
/// 1. Rust bot detects cookie error from yt-dlp
/// 2. Calls report_error() to notify cookie_manager
/// 3. Cookie manager triggers emergency refresh
/// 4. Rust bot retries download with fresh cookies
#[derive(Debug, Clone)]
pub struct CookieManagerClient {
    base_url: String,
    client: reqwest::Client,
}

/// Cookie manager health response
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CookieHealthResponse {
    pub status: String, // "healthy", "degraded", "unhealthy"
    pub mode: String,   // "full", "degraded", "po_token", "offline"
    pub freshness_score: Option<i32>,
    pub freshness_reason: Option<String>,
    #[serde(default)]
    pub checks: CookieHealthChecks,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct CookieHealthChecks {
    #[serde(default)]
    pub cookies: CookieCheck,
    #[serde(default)]
    pub error_tracker: ErrorTrackerStatus,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct CookieCheck {
    pub exists: bool,
    pub valid: bool,
    pub required_count: i32,
    pub min_expiry_hours: Option<f64>,
    pub expired: Option<i32>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct ErrorTrackerStatus {
    pub emergency_mode: bool,
    pub errors_last_5min: i32,
    pub errors_last_1h: i32,
}

/// Response from /api/report_error
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ReportErrorResponse {
    pub success: bool,
    pub action: String, // "refresh_triggered", "cooldown", "ignored"
    pub emergency_mode: Option<bool>,
    pub recent_errors: Option<i32>,
    pub refresh_result: Option<RefreshResult>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct RefreshResult {
    pub success: bool,
    pub method: Option<String>,
    pub cookie_count: Option<i32>,
    pub error: Option<String>,
}

/// Response from /api/export_cookies
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ExportCookiesResponse {
    pub success: bool,
    pub cookie_count: Option<i32>,
    pub error: Option<String>,
}

impl Default for CookieManagerClient {
    fn default() -> Self {
        Self::new()
    }
}

impl CookieManagerClient {
    /// Create new client with default URL
    pub fn new() -> Self {
        Self::with_base_url(COOKIE_MANAGER_BASE_URL)
    }

    /// Create client with custom base URL
    pub fn with_base_url(base_url: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|e| {
                log::warn!("Failed to build HTTP client with timeout: {}, using default", e);
                reqwest::Client::new()
            });

        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
        }
    }

    /// Check if cookie manager is available
    pub async fn is_available(&self) -> bool {
        match timeout(
            Duration::from_secs(2),
            self.client.get(format!("{}/health", self.base_url)).send(),
        )
        .await
        {
            Ok(Ok(resp)) => resp.status().is_success() || resp.status().as_u16() == 503,
            _ => false,
        }
    }

    /// Get cookie manager health status
    ///
    /// Returns health info or None if cookie manager is not available.
    pub async fn health_check(&self) -> Option<CookieHealthResponse> {
        let url = format!("{}/health", self.base_url);

        match timeout(Duration::from_secs(5), self.client.get(&url).send()).await {
            Ok(Ok(resp)) => match resp.json::<CookieHealthResponse>().await {
                Ok(health) => Some(health),
                _ => {
                    log::warn!("Failed to parse cookie manager health response");
                    None
                }
            },
            Ok(Err(e)) => {
                log::debug!("Cookie manager health check failed: {}", e);
                None
            }
            Err(_) => {
                log::debug!("Cookie manager health check timed out");
                None
            }
        }
    }

    /// Report a cookie error to the cookie manager (Feedback Loop)
    ///
    /// This triggers emergency refresh if the error is cookie-related.
    /// Call this when yt-dlp returns InvalidCookies or BotDetection errors.
    ///
    /// # Arguments
    /// * `error_type` - Error type: "InvalidCookies", "BotDetection", etc.
    /// * `url` - URL that caused the error (for diagnostics)
    ///
    /// # Returns
    /// * `Ok(response)` - Cookie manager received the error
    /// * `Err` - Failed to communicate with cookie manager
    pub async fn report_error(&self, error_type: &str, url: &str) -> Result<ReportErrorResponse> {
        let api_url = format!("{}/api/report_error", self.base_url);

        let body = serde_json::json!({
            "error_type": error_type,
            "url": url,
        });

        log::info!(
            "Reporting cookie error to manager: type={}, url={}",
            error_type,
            &url[..url.len().min(80)]
        );

        let resp = timeout(Duration::from_secs(180), self.client.post(&api_url).json(&body).send())
            .await
            .map_err(|_| anyhow::anyhow!("Cookie manager report_error timed out"))?
            .map_err(|e| anyhow::anyhow!("Cookie manager request failed: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("Cookie manager returned {}: {}", status, text));
        }

        let response: ReportErrorResponse = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))?;

        log::info!(
            "Cookie manager response: action={}, success={}, emergency={}",
            response.action,
            response.refresh_result.as_ref().map(|r| r.success).unwrap_or(false),
            response.emergency_mode.unwrap_or(false)
        );

        Ok(response)
    }

    /// Trigger cookie export/refresh
    ///
    /// Use this to manually request a cookie refresh from the browser.
    pub async fn trigger_refresh(&self) -> Result<ExportCookiesResponse> {
        let url = format!("{}/api/export_cookies", self.base_url);

        log::info!("Triggering cookie refresh via cookie manager");

        let resp = timeout(Duration::from_secs(180), self.client.post(&url).send())
            .await
            .map_err(|_| anyhow::anyhow!("Cookie manager refresh timed out"))?
            .map_err(|e| anyhow::anyhow!("Cookie manager request failed: {}", e))?;

        let response: ExportCookiesResponse = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))?;

        if response.success {
            log::info!(
                "Cookie refresh successful: {} cookies",
                response.cookie_count.unwrap_or(0)
            );
        } else {
            log::warn!("Cookie refresh failed: {:?}", response.error);
        }

        Ok(response)
    }

    /// Check if cookies are healthy (quick check)
    ///
    /// Returns true if:
    /// - Cookie manager is available AND status is "healthy"
    /// - OR cookie manager is not available (fallback to direct cookie usage)
    pub async fn is_healthy(&self) -> bool {
        match self.health_check().await {
            Some(health) => health.status == "healthy",
            None => {
                // If cookie manager is not available, assume cookies are okay
                // (they will be validated by yt-dlp anyway)
                true
            }
        }
    }

    /// Check if emergency mode is active
    pub async fn is_emergency_mode(&self) -> bool {
        self.health_check()
            .await
            .map(|h| h.checks.error_tracker.emergency_mode)
            .unwrap_or(false)
    }
}

/// Global singleton for cookie manager client
static COOKIE_MANAGER_CLIENT: std::sync::LazyLock<CookieManagerClient> =
    std::sync::LazyLock::new(CookieManagerClient::new);

/// Get the global cookie manager client
pub fn cookie_manager() -> &'static CookieManagerClient {
    &COOKIE_MANAGER_CLIENT
}

/// Report cookie error and wait for refresh (convenience function)
///
/// This is the main function to call when yt-dlp returns a cookie error.
/// It:
/// 1. Reports the error to cookie manager
/// 2. Waits for emergency refresh if triggered
/// 3. Returns success status
///
/// # Arguments
/// * `error_type` - Error type from ytdlp_errors module
/// * `url` - URL that caused the error
///
/// # Returns
/// * `true` if refresh was triggered and succeeded (caller should retry)
/// * `false` if refresh failed or was not triggered (caller should not retry)
pub async fn report_and_wait_for_refresh(error_type: &str, url: &str) -> bool {
    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
    log::warn!(
        "[COOKIE_EVENT:{}] Cookie error detected! type={} url={}",
        timestamp,
        error_type,
        &url[..url.len().min(100)]
    );

    // Snapshot cookie file state BEFORE refresh attempt
    log_cookie_file_diagnostics(&format!("BEFORE_REFRESH({})", error_type));

    let client = cookie_manager();

    // Check if cookie manager is available
    if !client.is_available().await {
        log::warn!(
            "[COOKIE_EVENT:{}] Cookie manager NOT available (http://127.0.0.1:4417 unreachable). Cannot refresh cookies.",
            timestamp
        );
        return false;
    }

    // Report error
    match client.report_error(error_type, url).await {
        Ok(response) => {
            log::warn!(
                "[COOKIE_EVENT:{}] Cookie manager response: action={} emergency={} recent_errors={} refresh_success={} refresh_method={}",
                timestamp,
                response.action,
                response.emergency_mode.unwrap_or(false),
                response.recent_errors.unwrap_or(0),
                response.refresh_result.as_ref().map(|r| r.success).unwrap_or(false),
                response
                    .refresh_result
                    .as_ref()
                    .and_then(|r| r.method.as_deref())
                    .unwrap_or("none"),
            );

            if response.action == "refresh_triggered" {
                if let Some(ref result) = response.refresh_result {
                    if result.success {
                        log::warn!(
                            "[COOKIE_EVENT:{}] Refresh SUCCESS via {} ({} cookies). Will retry download.",
                            timestamp,
                            result.method.as_deref().unwrap_or("unknown"),
                            result.cookie_count.unwrap_or(0),
                        );
                        // Snapshot cookie file state AFTER successful refresh
                        log_cookie_file_diagnostics("AFTER_REFRESH_SUCCESS");
                        return true;
                    } else {
                        log::error!(
                            "[COOKIE_EVENT:{}] Refresh FAILED: {}",
                            timestamp,
                            result.error.as_deref().unwrap_or("unknown error"),
                        );
                        log_cookie_file_diagnostics("AFTER_REFRESH_FAILURE");
                    }
                }
            } else if response.action == "cooldown" {
                log::warn!(
                    "[COOKIE_EVENT:{}] Refresh on COOLDOWN (recent refresh already happened). Waiting 5s...",
                    timestamp,
                );
                tokio::time::sleep(Duration::from_secs(5)).await;
                log_cookie_file_diagnostics("AFTER_COOLDOWN_WAIT");
                return true;
            } else if response.action == "ignored" {
                log::warn!(
                    "[COOKIE_EVENT:{}] Cookie manager IGNORED error (not classified as cookie error)",
                    timestamp,
                );
            }
        }
        Err(e) => {
            log::error!(
                "[COOKIE_EVENT:{}] Failed to communicate with cookie manager: {}",
                timestamp,
                e
            );
        }
    }

    false
}
