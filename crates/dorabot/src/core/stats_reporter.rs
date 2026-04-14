//! Periodic statistics reporter
//!
//! Sends periodic statistics reports to the admin via Telegram.
//! Reports include download counts, message counts, success/failure rates,
//! and file type breakdown.

use crate::storage::db::{self};
use crate::storage::SharedStorage;
use crate::telegram::admin;
use crate::telegram::Bot;
use crate::telegram::BotExt;
use anyhow::Context;
use chrono::{Duration, Utc};
use sqlx::{pool::PoolConnection, Postgres, Row};
use std::sync::Arc;
use teloxide::types::ChatId;

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

/// Gets statistics for the last N hours
pub async fn get_period_stats(shared_storage: &Arc<SharedStorage>, hours: i64) -> anyhow::Result<PeriodStats> {
    let cutoff = Utc::now() - Duration::hours(hours);
    let cutoff_rfc3339 = cutoff.to_rfc3339();

    let download_entries: Vec<db::DownloadHistoryEntry> = match shared_storage.as_ref() {
        SharedStorage::Sqlite { db_pool } => {
            let conn = db::get_connection(db_pool)?;
            let mut stmt = conn.prepare(
                "SELECT id, url, title, format, downloaded_at, file_id, author,
                        file_size, duration, video_quality, audio_bitrate, bot_api_url, bot_api_is_local,
                        source_id, part_index, category, speed
                 FROM download_history
                 WHERE downloaded_at >= ?1
                 ORDER BY downloaded_at DESC",
            )?;
            let rows = stmt.query_map([&cutoff_rfc3339], |row| {
                Ok(db::DownloadHistoryEntry {
                    id: row.get(0)?,
                    url: row.get(1)?,
                    title: row.get(2)?,
                    format: row.get(3)?,
                    downloaded_at: row.get(4)?,
                    file_id: row.get(5)?,
                    author: row.get(6)?,
                    file_size: row.get(7)?,
                    duration: row.get(8)?,
                    video_quality: row.get(9)?,
                    audio_bitrate: row.get(10)?,
                    bot_api_url: row.get(11)?,
                    bot_api_is_local: row.get(12)?,
                    source_id: row.get(13)?,
                    part_index: row.get(14)?,
                    category: row.get(15)?,
                    speed: row.get(16)?,
                })
            })?;
            rows.filter_map(|r| match r {
                Ok(entry) => Some(entry),
                Err(e) => {
                    log::warn!("Skipping malformed download_history row: {}", e);
                    None
                }
            })
            .collect()
        }
        SharedStorage::Postgres { pg_pool, .. } => {
            let rows = sqlx::query(
                "SELECT id, url, title, format, downloaded_at::text AS downloaded_at, file_id, author,
                        file_size, duration, video_quality, audio_bitrate, bot_api_url, bot_api_is_local,
                        source_id, part_index, category, speed
                 FROM download_history
                 WHERE downloaded_at >= $1::timestamptz
                 ORDER BY downloaded_at DESC",
            )
            .bind(&cutoff_rfc3339)
            .fetch_all(pg_pool)
            .await?;
            rows.into_iter()
                .map(|row| db::DownloadHistoryEntry {
                    id: row.get("id"),
                    url: row.get("url"),
                    title: row.get("title"),
                    format: row.get("format"),
                    downloaded_at: row.get("downloaded_at"),
                    file_id: row.get("file_id"),
                    author: row.get("author"),
                    file_size: row.get("file_size"),
                    duration: row.get("duration"),
                    video_quality: row.get("video_quality"),
                    audio_bitrate: row.get("audio_bitrate"),
                    bot_api_url: row.get("bot_api_url"),
                    bot_api_is_local: row.get("bot_api_is_local"),
                    source_id: row.get("source_id"),
                    part_index: row.get("part_index"),
                    category: row.get("category"),
                    speed: row.try_get::<Option<f32>, _>("speed").ok().flatten(),
                })
                .collect()
        }
    };

    let total_downloads = download_entries.len() as i64;
    let unique_users =
        match shared_storage.as_ref() {
            SharedStorage::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool)?;
                conn.query_row(
                    "SELECT COUNT(DISTINCT user_id) FROM download_history WHERE downloaded_at >= ?1",
                    [&cutoff_rfc3339],
                    |row| row.get(0),
                )?
            }
            SharedStorage::Postgres { pg_pool, .. } => sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(DISTINCT user_id)::bigint FROM download_history WHERE downloaded_at >= $1::timestamptz",
            )
            .bind(&cutoff_rfc3339)
            .fetch_one(pg_pool)
            .await?,
        };
    let total_size = download_entries.iter().map(|entry| entry.file_size.unwrap_or(0)).sum();
    let mut by_format_map = std::collections::BTreeMap::<String, i64>::new();
    for entry in &download_entries {
        *by_format_map.entry(entry.format.clone()).or_default() += 1;
    }
    let by_format = by_format_map.into_iter().collect::<Vec<_>>();

    let failed_downloads = match shared_storage.as_ref() {
        SharedStorage::Sqlite { db_pool } => {
            let conn = db::get_connection(db_pool)?;
            conn.query_row(
                "SELECT COUNT(*) FROM task_queue WHERE status = 'dead_letter' AND updated_at >= ?1",
                [&cutoff_rfc3339],
                |row| row.get(0),
            )?
        }
        SharedStorage::Postgres { pg_pool, .. } => sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::bigint FROM task_queue WHERE status = 'dead_letter' AND updated_at >= $1::timestamptz",
        )
        .bind(&cutoff_rfc3339)
        .fetch_one(pg_pool)
        .await?,
    };

    let errors_by_type = match shared_storage.get_error_stats(hours).await {
        Ok(v) => v,
        Err(e) => {
            log::error!("Failed to fetch error stats: {}", e);
            Vec::new()
        }
    };
    let recent_errors = match shared_storage.get_recent_errors(hours, 5).await {
        Ok(v) => v,
        Err(e) => {
            log::error!("Failed to fetch recent errors: {}", e);
            Vec::new()
        }
    };

    Ok(PeriodStats {
        total_downloads,
        successful_downloads: total_downloads,
        failed_downloads,
        unique_users,
        by_format,
        total_size,
        errors_by_type,
        recent_errors,
    })
}

/// Formats size in human-readable format.
/// Thin re-export of `doracore::core::format_bytes_i64` — keep this file's
/// call sites unchanged.
fn format_size(bytes: i64) -> String {
    doracore::core::format_bytes_i64(bytes)
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
    shared_storage: Arc<SharedStorage>,
    interval_hours: u64,
}

impl StatsReporter {
    /// Creates a new stats reporter
    pub fn new(bot: Bot, admin_chat_id: ChatId, shared_storage: Arc<SharedStorage>, interval_hours: u64) -> Self {
        Self {
            bot,
            admin_chat_id,
            shared_storage,
            interval_hours,
        }
    }

    /// Sends a stats report for the last N hours
    pub async fn send_report(&self, hours: i64) -> anyhow::Result<()> {
        let stats = get_period_stats(&self.shared_storage, hours)
            .await
            .with_context(|| "Stats error")?;

        // Skip if no activity
        if stats.total_downloads == 0 && stats.failed_downloads == 0 {
            log::debug!("No activity in last {} hours, skipping stats report", hours);
            return Ok(());
        }

        let message = format_stats_message(&stats, hours);

        self.bot
            .send_md(self.admin_chat_id, &message)
            .await
            .with_context(|| "Failed to send stats")?;

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
    shared_storage: Arc<SharedStorage>,
    interval_hours: u64,
    lock_conn: Option<PoolConnection<Postgres>>,
) -> Arc<StatsReporter> {
    let reporter = Arc::new(StatsReporter::new(bot, admin_chat_id, shared_storage, interval_hours));

    let reporter_clone = Arc::clone(&reporter);
    tokio::spawn(async move {
        let _lock_conn = lock_conn;
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
