use anyhow::{Context, Result};
use sqlx::Row;

use crate::storage::db::{self, DownloadHistoryEntry, GlobalStats, UserStats};

use super::SharedStorage;
use super::download_history::map_pg_download_history;

impl SharedStorage {
    pub async fn save_feedback(
        &self,
        user_id: i64,
        username: Option<&str>,
        first_name: &str,
        message: &str,
    ) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite save_feedback connection")?;
                db::save_feedback(&conn, user_id, username, first_name, message).context("sqlite save_feedback")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "INSERT INTO feedback_messages (user_id, username, first_name, message, status)
                     VALUES ($1, $2, $3, $4, 'new')
                     RETURNING id",
                )
                .bind(user_id)
                .bind(username)
                .bind(first_name)
                .bind(message)
                .fetch_one(pg_pool)
                .await
                .context("postgres save_feedback")?;
                Ok(row.get::<i64, _>("id"))
            }
        }
    }

    pub async fn get_user_stats(&self, telegram_id: i64) -> Result<UserStats> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user_stats connection")?;
                db::get_user_stats(&conn, telegram_id).context("sqlite get_user_stats")
            }
            Self::Postgres { pg_pool, .. } => {
                let total_downloads =
                    sqlx::query_scalar::<_, i64>("SELECT COUNT(*)::bigint FROM download_history WHERE user_id = $1")
                        .bind(telegram_id)
                        .fetch_one(pg_pool)
                        .await
                        .context("postgres get_user_stats total_downloads")?;

                let total_size = sqlx::query_scalar::<_, Option<i64>>(
                    "SELECT SUM(
                        CASE
                            WHEN format = 'mp3' THEN 5000000
                            WHEN format = 'mp4' THEN 50000000
                            ELSE 1000000
                        END
                    )::bigint
                     FROM download_history
                     WHERE user_id = $1",
                )
                .bind(telegram_id)
                .fetch_one(pg_pool)
                .await
                .context("postgres get_user_stats total_size")?
                .unwrap_or(0);

                let active_days = sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(DISTINCT DATE(downloaded_at))::bigint
                     FROM download_history
                     WHERE user_id = $1",
                )
                .bind(telegram_id)
                .fetch_one(pg_pool)
                .await
                .context("postgres get_user_stats active_days")?;

                let title_rows = sqlx::query(
                    "SELECT title
                     FROM download_history
                     WHERE user_id = $1
                     ORDER BY downloaded_at DESC
                     LIMIT 100",
                )
                .bind(telegram_id)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_user_stats titles")?;

                let mut artist_counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
                for row in title_rows {
                    let title: String = row.get("title");
                    if let Some(pos) = title.find(" - ") {
                        let artist = title[..pos].trim().to_string();
                        if !artist.is_empty() {
                            *artist_counts.entry(artist).or_insert(0) += 1;
                        }
                    }
                }

                let mut top_artists: Vec<(String, i64)> = artist_counts.into_iter().collect();
                top_artists.sort_by_key(|b| std::cmp::Reverse(b.1));
                top_artists.truncate(5);

                let format_rows = sqlx::query(
                    "SELECT format, COUNT(*)::bigint AS cnt
                     FROM download_history
                     WHERE user_id = $1
                     GROUP BY format
                     ORDER BY cnt DESC
                     LIMIT 5",
                )
                .bind(telegram_id)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_user_stats top_formats")?;
                let top_formats = format_rows
                    .into_iter()
                    .map(|row| (row.get::<String, _>("format"), row.get::<i64, _>("cnt")))
                    .collect();

                let activity_rows = sqlx::query(
                    "SELECT DATE(downloaded_at)::text AS day, COUNT(*)::bigint AS cnt
                     FROM download_history
                     WHERE user_id = $1
                       AND downloaded_at >= NOW() - INTERVAL '7 days'
                     GROUP BY DATE(downloaded_at)
                     ORDER BY day DESC",
                )
                .bind(telegram_id)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_user_stats activity_by_day")?;
                let activity_by_day = activity_rows
                    .into_iter()
                    .map(|row| (row.get::<String, _>("day"), row.get::<i64, _>("cnt")))
                    .collect();

                Ok(UserStats {
                    total_downloads,
                    total_size,
                    active_days,
                    top_artists,
                    top_formats,
                    activity_by_day,
                })
            }
        }
    }

    pub async fn get_global_stats(&self) -> Result<GlobalStats> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_global_stats connection")?;
                db::get_global_stats(&conn).context("sqlite get_global_stats")
            }
            Self::Postgres { pg_pool, .. } => {
                let total_users =
                    sqlx::query_scalar::<_, i64>("SELECT COUNT(DISTINCT user_id)::bigint FROM download_history")
                        .fetch_one(pg_pool)
                        .await
                        .context("postgres get_global_stats total_users")?;

                let total_downloads = sqlx::query_scalar::<_, i64>("SELECT COUNT(*)::bigint FROM download_history")
                    .fetch_one(pg_pool)
                    .await
                    .context("postgres get_global_stats total_downloads")?;

                let top_track_rows = sqlx::query(
                    "SELECT title, COUNT(*)::bigint AS cnt
                     FROM download_history
                     GROUP BY title
                     ORDER BY cnt DESC
                     LIMIT 10",
                )
                .fetch_all(pg_pool)
                .await
                .context("postgres get_global_stats top_tracks")?;
                let top_tracks = top_track_rows
                    .into_iter()
                    .map(|row| (row.get::<String, _>("title"), row.get::<i64, _>("cnt")))
                    .collect();

                let top_format_rows = sqlx::query(
                    "SELECT format, COUNT(*)::bigint AS cnt
                     FROM download_history
                     GROUP BY format
                     ORDER BY cnt DESC",
                )
                .fetch_all(pg_pool)
                .await
                .context("postgres get_global_stats top_formats")?;
                let top_formats = top_format_rows
                    .into_iter()
                    .map(|row| (row.get::<String, _>("format"), row.get::<i64, _>("cnt")))
                    .collect();

                Ok(GlobalStats {
                    total_users,
                    total_downloads,
                    top_tracks,
                    top_formats,
                })
            }
        }
    }

    pub async fn get_all_download_history(&self, telegram_id: i64) -> Result<Vec<DownloadHistoryEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_all_download_history connection")?;
                db::get_all_download_history(&conn, telegram_id).context("sqlite get_all_download_history")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, url, title, format, downloaded_at::text AS downloaded_at, file_id, author,
                            file_size, duration, video_quality, audio_bitrate, bot_api_url, bot_api_is_local,
                            source_id, part_index, category
                     FROM download_history
                     WHERE user_id = $1
                     ORDER BY downloaded_at DESC",
                )
                .bind(telegram_id)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_all_download_history")?;
                Ok(rows.into_iter().map(map_pg_download_history).collect())
            }
        }
    }

    pub async fn count_daily_active_users(&self) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite count_daily_active_users connection")?;
                let count = conn
                    .query_row(
                        "SELECT COUNT(DISTINCT user_id) FROM request_history WHERE date(timestamp) = date('now')",
                        [],
                        |row| row.get(0),
                    )
                    .context("sqlite count_daily_active_users")?;
                Ok(count)
            }
            Self::Postgres { pg_pool, .. } => {
                let count = sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(DISTINCT user_id)::bigint FROM request_history WHERE DATE(timestamp) = CURRENT_DATE",
                )
                .fetch_one(pg_pool)
                .await
                .context("postgres count_daily_active_users")?;
                Ok(count)
            }
        }
    }

    pub async fn count_monthly_active_users(&self) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite count_monthly_active_users connection")?;
                let count = conn
                    .query_row(
                        "SELECT COUNT(DISTINCT user_id) FROM request_history WHERE timestamp >= datetime('now', '-30 days')",
                        [],
                        |row| row.get(0),
                    )
                    .context("sqlite count_monthly_active_users")?;
                Ok(count)
            }
            Self::Postgres { pg_pool, .. } => {
                let count = sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(DISTINCT user_id)::bigint FROM request_history WHERE timestamp >= NOW() - INTERVAL '30 days'",
                )
                .fetch_one(pg_pool)
                .await
                .context("postgres count_monthly_active_users")?;
                Ok(count)
            }
        }
    }
}
