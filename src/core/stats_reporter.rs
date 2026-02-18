//! Periodic statistics reporter
//!
//! Sends periodic statistics reports to the admin via Telegram.
//! Reports include download counts, message counts, success/failure rates,
//! and file type breakdown.

use crate::storage::db::{self, DbPool};
use crate::telegram::admin;
use crate::telegram::Bot;
use chrono::{Duration, Utc};
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{ChatId, ParseMode};

/// Statistics for a time period
#[derive(Debug, Clone, Default)]
pub struct PeriodStats {
    /// Total downloads in the period
    pub total_downloads: i64,
    /// Successful downloads
    pub successful_downloads: i64,
    /// Failed downloads
    pub failed_downloads: i64,
    /// Unique users who downloaded
    pub unique_users: i64,
    /// Downloads by format (format -> count)
    pub by_format: Vec<(String, i64)>,
    /// Total file size downloaded (bytes)
    pub total_size: i64,
    /// Errors by type (error_type -> count)
    pub errors_by_type: Vec<(String, i64)>,
    /// Recent errors (up to 5)
    pub recent_errors: Vec<db::ErrorLogEntry>,
}

/// Helper to log database query errors and return default value
fn query_with_logging<T: Default>(result: Result<T, rusqlite::Error>, query_name: &str) -> T {
    match result {
        Ok(value) => value,
        Err(e) => {
            log::warn!("Stats query '{}' failed: {}", query_name, e);
            T::default()
        }
    }
}

/// Gets statistics for the last N hours
pub fn get_period_stats(conn: &db::DbConnection, hours: i64) -> anyhow::Result<PeriodStats> {
    let since = Utc::now() - Duration::hours(hours);
    let since_str = since.format("%Y-%m-%d %H:%M:%S").to_string();

    // Total downloads in period
    let total_downloads: i64 = query_with_logging(
        conn.query_row(
            "SELECT COUNT(*) FROM download_history WHERE downloaded_at >= ?",
            [&since_str],
            |row| row.get(0),
        ),
        "total_downloads",
    );

    // Unique users
    let unique_users: i64 = query_with_logging(
        conn.query_row(
            "SELECT COUNT(DISTINCT user_id) FROM download_history WHERE downloaded_at >= ?",
            [&since_str],
            |row| row.get(0),
        ),
        "unique_users",
    );

    // Total size
    let total_size: i64 = query_with_logging(
        conn.query_row(
            "SELECT COALESCE(SUM(file_size), 0) FROM download_history WHERE downloaded_at >= ?",
            [&since_str],
            |row| row.get(0),
        ),
        "total_size",
    );

    // By format
    let mut stmt = conn.prepare(
        "SELECT format, COUNT(*) as cnt FROM download_history
         WHERE downloaded_at >= ?
         GROUP BY format ORDER BY cnt DESC",
    )?;
    let rows = stmt.query_map([&since_str], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;

    let mut by_format = Vec::new();
    for row in rows.flatten() {
        by_format.push(row);
    }

    // Failed downloads from task_queue
    let failed_downloads: i64 = query_with_logging(
        conn.query_row(
            "SELECT COUNT(*) FROM task_queue WHERE status = 'failed' AND updated_at >= ?",
            [&since_str],
            |row| row.get(0),
        ),
        "failed_downloads",
    );

    // Get errors by type from error_log
    let errors_by_type = db::get_error_stats(conn, hours).unwrap_or_else(|e| {
        log::warn!("Stats query 'errors_by_type' failed: {}", e);
        Vec::new()
    });

    // Get recent errors (last 5)
    let recent_errors = db::get_recent_errors(conn, hours, 5).unwrap_or_else(|e| {
        log::warn!("Stats query 'recent_errors' failed: {}", e);
        Vec::new()
    });

    Ok(PeriodStats {
        total_downloads,
        successful_downloads: total_downloads, // download_history only has successful ones
        failed_downloads,
        unique_users,
        by_format,
        total_size,
        errors_by_type,
        recent_errors,
    })
}

/// Formats size in human-readable format
fn format_size(bytes: i64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

/// Formats the period stats as a Telegram message
fn format_stats_message(stats: &PeriodStats, hours: i64) -> String {
    let mut text = format!("📊 *Stats for the last {} h\\.*\n\n", hours);

    // Downloads summary
    let success_rate = if stats.total_downloads + stats.failed_downloads > 0 {
        (stats.successful_downloads as f64 / (stats.total_downloads + stats.failed_downloads) as f64) * 100.0
    } else {
        100.0
    };

    text.push_str(&format!(
        "📥 Downloads: {} \\(✅ {}, ❌ {}\\)\n",
        stats.total_downloads + stats.failed_downloads,
        stats.successful_downloads,
        stats.failed_downloads
    ));

    text.push_str(&format!("📈 Success rate: {:.1}%\n", success_rate).replace('.', "\\."));

    text.push_str(&format!("👥 Unique users: {}\n", stats.unique_users));

    text.push_str(&format!(
        "💾 Volume: {}\n\n",
        admin::escape_markdown(&format_size(stats.total_size))
    ));

    // By format
    if !stats.by_format.is_empty() {
        text.push_str("*By type:*\n");
        for (format, count) in &stats.by_format {
            let emoji = match format.as_str() {
                "mp3" => "🎵",
                "mp4" => "🎬",
                "video_note" => "⚪",
                "srt" => "📝",
                "txt" => "📄",
                _ => "📦",
            };
            let display_format = if format == "video_note" {
                "Video note"
            } else {
                &format.to_uppercase()
            };
            text.push_str(&format!(
                "  {} {}: {}\n",
                emoji,
                admin::escape_markdown(display_format),
                count
            ));
        }
    }

    // Errors section
    if !stats.errors_by_type.is_empty() {
        text.push_str("\n*Errors:*\n");
        for (error_type, count) in &stats.errors_by_type {
            let emoji = match error_type.as_str() {
                "download_failed" => "📥",
                "file_too_large" => "📦",
                "timeout" => "⏱️",
                "mtproto_error" => "🔌",
                "telegram_api_error" => "📱",
                "ffmpeg_error" => "🎬",
                "invalid_url" => "🔗",
                _ => "❓",
            };
            text.push_str(&format!(
                "  {} {}: {}\n",
                emoji,
                admin::escape_markdown(error_type),
                count
            ));
        }
    }

    // Recent errors with user info
    if !stats.recent_errors.is_empty() {
        text.push_str("\n*Recent errors:*\n");
        for error in &stats.recent_errors {
            let user_display = if let Some(ref username) = error.username {
                format!("@{}", username)
            } else if let Some(user_id) = error.user_id {
                format!("id:{}", user_id)
            } else {
                "unknown".to_string()
            };

            // Truncate error message to 50 chars
            let msg_preview: String = error.error_message.chars().take(50).collect();
            let suffix = if error.error_message.len() > 50 {
                "\\.\\.\\."
            } else {
                ""
            };

            text.push_str(&format!(
                "  • {} \\- {}{}\n",
                admin::escape_markdown(&user_display),
                admin::escape_markdown(&msg_preview),
                suffix
            ));
        }
    }

    text
}

/// Stats reporter that sends periodic reports to admin
pub struct StatsReporter {
    bot: Bot,
    admin_chat_id: ChatId,
    db_pool: Arc<DbPool>,
    interval_hours: u64,
}

impl StatsReporter {
    /// Creates a new stats reporter
    pub fn new(bot: Bot, admin_chat_id: ChatId, db_pool: Arc<DbPool>, interval_hours: u64) -> Self {
        Self {
            bot,
            admin_chat_id,
            db_pool,
            interval_hours,
        }
    }

    /// Sends a stats report for the last N hours
    pub async fn send_report(&self, hours: i64) -> Result<(), String> {
        let conn = db::get_connection(&self.db_pool).map_err(|e| format!("DB error: {}", e))?;

        let stats = get_period_stats(&conn, hours).map_err(|e| format!("Stats error: {}", e))?;

        // Skip if no activity
        if stats.total_downloads == 0 && stats.failed_downloads == 0 {
            log::debug!("No activity in last {} hours, skipping stats report", hours);
            return Ok(());
        }

        let message = format_stats_message(&stats, hours);

        self.bot
            .send_message(self.admin_chat_id, &message)
            .parse_mode(ParseMode::MarkdownV2)
            .await
            .map_err(|e| format!("Failed to send stats: {:?}", e))?;

        log::info!(
            "Sent stats report: {} downloads, {} users in last {} hours",
            stats.total_downloads,
            stats.unique_users,
            hours
        );

        Ok(())
    }

    /// Gets the interval in hours
    pub fn interval_hours(&self) -> u64 {
        self.interval_hours
    }
}

/// Starts the periodic stats reporter background task
///
/// Sends statistics every `interval_hours` to the admin.
pub fn start_stats_reporter(
    bot: Bot,
    admin_chat_id: ChatId,
    db_pool: Arc<DbPool>,
    interval_hours: u64,
) -> Arc<StatsReporter> {
    let reporter = Arc::new(StatsReporter::new(bot, admin_chat_id, db_pool, interval_hours));

    let reporter_clone = Arc::clone(&reporter);
    tokio::spawn(async move {
        let interval_secs = interval_hours * 3600;
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(interval_secs));

        // Skip the first immediate tick
        interval.tick().await;

        loop {
            interval.tick().await;

            if let Err(e) = reporter_clone.send_report(interval_hours as i64).await {
                log::error!("Stats report error: {}", e);
            }
        }
    });

    log::info!(
        "Stats reporter started (sending every {} hours to admin {})",
        interval_hours,
        admin_chat_id.0
    );

    reporter
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(500), "500 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_size(1024 * 1024 * 1024), "1.00 GB");
    }

    #[test]
    fn test_format_stats_message() {
        let stats = PeriodStats {
            total_downloads: 100,
            successful_downloads: 100,
            failed_downloads: 5,
            unique_users: 10,
            by_format: vec![
                ("mp3".to_string(), 50),
                ("mp4".to_string(), 30),
                ("video_note".to_string(), 20),
            ],
            total_size: 1024 * 1024 * 500, // 500 MB
            errors_by_type: vec![("download_failed".to_string(), 3), ("timeout".to_string(), 2)],
            recent_errors: vec![],
        };

        let message = format_stats_message(&stats, 3);
        assert!(message.contains("Stats for the last 3"));
        assert!(message.contains("Downloads"));
        assert!(message.contains("mp3") || message.contains("MP3"));
    }
}
