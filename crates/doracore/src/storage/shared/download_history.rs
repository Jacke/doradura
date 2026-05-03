use anyhow::{Context, Result};
use sqlx::Row;

use crate::core::config;
use crate::storage::db::{self, CutEntry, DownloadHistoryEntry};
use crate::timestamps::{TimestampSource, VideoTimestamp};

use super::SharedStorage;

impl SharedStorage {
    #[allow(clippy::too_many_arguments)]
    pub async fn save_download_history(
        &self,
        telegram_id: i64,
        url: &str,
        title: &str,
        format: &str,
        file_id: Option<&str>,
        author: Option<&str>,
        file_size: Option<i64>,
        duration: Option<i64>,
        video_quality: Option<&str>,
        audio_bitrate: Option<&str>,
        source_id: Option<i64>,
        part_index: Option<i32>,
        speed: Option<f32>,
    ) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite save_download_history connection")?;
                db::save_download_history(
                    &conn,
                    telegram_id,
                    url,
                    title,
                    format,
                    file_id,
                    author,
                    file_size,
                    duration,
                    video_quality,
                    audio_bitrate,
                    source_id,
                    part_index,
                    speed,
                )
                .context("sqlite save_download_history")
            }
            Self::Postgres { pg_pool, .. } => {
                let bot_api_url = config::bot_api::get_url();
                let bot_api_is_local = if config::bot_api::is_local() { 1 } else { 0 };
                let row = sqlx::query(
                    "INSERT INTO download_history (
                        user_id, url, title, format, file_id, author, file_size, duration,
                        video_quality, audio_bitrate, bot_api_url, bot_api_is_local, source_id, part_index, speed
                     ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
                     RETURNING id",
                )
                .bind(telegram_id)
                .bind(url)
                .bind(title)
                .bind(format)
                .bind(file_id)
                .bind(author)
                .bind(file_size)
                .bind(duration)
                .bind(video_quality)
                .bind(audio_bitrate)
                .bind(bot_api_url)
                .bind(bot_api_is_local)
                .bind(source_id)
                .bind(part_index)
                .bind(speed)
                .fetch_one(pg_pool)
                .await
                .context("postgres save_download_history")?;
                Ok(row.get("id"))
            }
        }
    }

    pub async fn update_download_message_id(&self, download_id: i64, message_id: i32, chat_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite update_download_message_id connection")?;
                db::update_download_message_id(&conn, download_id, message_id, chat_id)
                    .context("sqlite update_download_message_id")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("UPDATE download_history SET message_id = $2, chat_id = $3 WHERE id = $1")
                    .bind(download_id)
                    .bind(message_id)
                    .bind(chat_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres update_download_message_id")?;
                Ok(())
            }
        }
    }

    pub async fn get_download_message_info(&self, download_id: i64) -> Result<Option<(i32, i64)>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_download_message_info connection")?;
                db::get_download_message_info(&conn, download_id).context("sqlite get_download_message_info")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT message_id, chat_id FROM download_history
                     WHERE id = $1 AND message_id IS NOT NULL AND chat_id IS NOT NULL",
                )
                .bind(download_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_download_message_info")?;
                Ok(row.map(|row| (row.get("message_id"), row.get("chat_id"))))
            }
        }
    }

    pub async fn get_download_history(
        &self,
        telegram_id: i64,
        limit: Option<i32>,
    ) -> Result<Vec<DownloadHistoryEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_download_history connection")?;
                db::get_download_history(&conn, telegram_id, limit).context("sqlite get_download_history")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, url, title, format, downloaded_at::text AS downloaded_at, file_id, author,
                            file_size, duration, video_quality, audio_bitrate, bot_api_url, bot_api_is_local,
                            source_id, part_index, category, speed
                     FROM download_history
                     WHERE user_id = $1
                     ORDER BY downloaded_at DESC
                     LIMIT $2",
                )
                .bind(telegram_id)
                .bind(i64::from(limit.unwrap_or(20)))
                .fetch_all(pg_pool)
                .await
                .context("postgres get_download_history")?;
                Ok(rows.into_iter().map(map_pg_download_history).collect())
            }
        }
    }

    /// Same lookup criteria as [`Self::find_cached_file_id`] but also returns
    /// the cached `title` and `author` (artist) so callers serving from
    /// cache can re-hydrate metadata they'd otherwise need yt-dlp for.
    ///
    /// Used by the lyrics path: when a download is served from cache, the
    /// lyrics fetch needs `(artist, title)` to query Genius/LRCLIB —
    /// without these, lyrics silently never appear.
    pub async fn find_cached_file_id_with_meta(
        &self,
        url: &str,
        format: &str,
        video_quality: Option<&str>,
        audio_bitrate: Option<&str>,
    ) -> Result<Option<(String, String, String)>> {
        match self {
            Self::Sqlite { .. } => {
                // Sqlite path: fall back to the file_id-only lookup (rare
                // dev-only branch — not worth a parallel full SQL surface).
                Ok(self
                    .find_cached_file_id(url, format, video_quality, audio_bitrate)
                    .await?
                    .map(|fid| (fid, String::new(), String::new())))
            }
            Self::Postgres { pg_pool, .. } => {
                let current_api_url = std::env::var("BOT_API_URL").ok();
                let current_is_local = current_api_url
                    .as_deref()
                    .map(|u| !u.contains("api.telegram.org"))
                    .unwrap_or(false);
                let row = sqlx::query(
                    "SELECT file_id, title, COALESCE(author, '') AS author
                     FROM download_history
                     WHERE url = $1
                       AND format = $2
                       AND file_id IS NOT NULL
                       AND bot_api_is_local = $3
                       AND ($4::text IS NULL OR video_quality = $4)
                       AND ($5::text IS NULL OR audio_bitrate = $5)
                       AND ($6::text IS NULL OR bot_api_url = $6)
                     ORDER BY downloaded_at DESC
                     LIMIT 1",
                )
                .bind(url)
                .bind(format)
                .bind(i32::from(current_is_local))
                .bind(video_quality)
                .bind(audio_bitrate)
                .bind(current_api_url)
                .fetch_optional(pg_pool)
                .await
                .context("postgres find_cached_file_id_with_meta")?;
                Ok(row.map(|row| {
                    (
                        row.get::<String, _>("file_id"),
                        row.get::<String, _>("title"),
                        row.get::<String, _>("author"),
                    )
                }))
            }
        }
    }

    pub async fn find_cached_file_id(
        &self,
        url: &str,
        format: &str,
        video_quality: Option<&str>,
        audio_bitrate: Option<&str>,
    ) -> Result<Option<String>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite find_cached_file_id connection")?;
                db::find_cached_file_id(&conn, url, format, video_quality, audio_bitrate)
                    .context("sqlite find_cached_file_id")
            }
            Self::Postgres { pg_pool, .. } => {
                let current_api_url = std::env::var("BOT_API_URL").ok();
                let current_is_local = current_api_url
                    .as_deref()
                    .map(|u| !u.contains("api.telegram.org"))
                    .unwrap_or(false);
                let row = sqlx::query(
                    "SELECT file_id
                     FROM download_history
                     WHERE url = $1
                       AND format = $2
                       AND file_id IS NOT NULL
                       AND bot_api_is_local = $3
                       AND ($4::text IS NULL OR video_quality = $4)
                       AND ($5::text IS NULL OR audio_bitrate = $5)
                       AND ($6::text IS NULL OR bot_api_url = $6)
                     ORDER BY downloaded_at DESC
                     LIMIT 1",
                )
                .bind(url)
                .bind(format)
                .bind(i32::from(current_is_local))
                .bind(video_quality)
                .bind(audio_bitrate)
                .bind(current_api_url)
                .fetch_optional(pg_pool)
                .await
                .context("postgres find_cached_file_id")?;
                Ok(row.map(|row| row.get("file_id")))
            }
        }
    }

    pub async fn get_download_history_entry(
        &self,
        telegram_id: i64,
        entry_id: i64,
    ) -> Result<Option<DownloadHistoryEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_download_history_entry connection")?;
                db::get_download_history_entry(&conn, telegram_id, entry_id)
                    .context("sqlite get_download_history_entry")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT id, url, title, format, downloaded_at::text AS downloaded_at, file_id, author,
                            file_size, duration, video_quality, audio_bitrate, bot_api_url, bot_api_is_local,
                            source_id, part_index, category, speed
                     FROM download_history
                     WHERE id = $1 AND user_id = $2",
                )
                .bind(entry_id)
                .bind(telegram_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_download_history_entry")?;
                Ok(row.map(map_pg_download_history))
            }
        }
    }

    pub async fn get_download_history_filtered(
        &self,
        user_id: i64,
        file_type_filter: Option<&str>,
        search_text: Option<&str>,
        category_filter: Option<&str>,
    ) -> Result<Vec<DownloadHistoryEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_download_history_filtered connection")?;
                db::get_download_history_filtered(&conn, user_id, file_type_filter, search_text, category_filter)
                    .context("sqlite get_download_history_filtered")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, url, title, format, downloaded_at::text AS downloaded_at, file_id, author,
                            file_size, duration, video_quality, audio_bitrate, bot_api_url, bot_api_is_local,
                            source_id, part_index, category, speed
                     FROM download_history
                     WHERE user_id = $1
                       AND file_id IS NOT NULL
                       AND (format = 'mp3' OR format = 'mp4')
                       AND ($2::text IS NULL OR format = $2)
                       AND ($3::text IS NULL OR (title ILIKE $3 OR author ILIKE $3))
                       AND ($4::text IS NULL OR category = $4)
                     ORDER BY downloaded_at DESC",
                )
                .bind(user_id)
                .bind(file_type_filter)
                .bind(search_text.map(|s| format!("%{}%", s)))
                .bind(category_filter)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_download_history_filtered")?;
                Ok(rows.into_iter().map(map_pg_download_history).collect())
            }
        }
    }

    pub async fn delete_download_history_entry(&self, telegram_id: i64, entry_id: i64) -> Result<bool> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite delete_download_history_entry connection")?;
                db::delete_download_history_entry(&conn, telegram_id, entry_id)
                    .context("sqlite delete_download_history_entry")
            }
            Self::Postgres { pg_pool, .. } => {
                let result = sqlx::query("DELETE FROM download_history WHERE id = $1 AND user_id = $2")
                    .bind(entry_id)
                    .bind(telegram_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres delete_download_history_entry")?;
                Ok(result.rows_affected() > 0)
            }
        }
    }

    pub async fn create_user_category(&self, user_id: i64, name: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite create_user_category connection")?;
                db::create_user_category(&conn, user_id, name).context("sqlite create_user_category")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO user_categories (user_id, name)
                     VALUES ($1, $2)
                     ON CONFLICT (user_id, name) DO NOTHING",
                )
                .bind(user_id)
                .bind(name)
                .execute(pg_pool)
                .await
                .context("postgres create_user_category")?;
                Ok(())
            }
        }
    }

    pub async fn get_user_categories(&self, user_id: i64) -> Result<Vec<String>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user_categories connection")?;
                db::get_user_categories(&conn, user_id).context("sqlite get_user_categories")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query("SELECT name FROM user_categories WHERE user_id = $1 ORDER BY name")
                    .bind(user_id)
                    .fetch_all(pg_pool)
                    .await
                    .context("postgres get_user_categories")?;
                Ok(rows.into_iter().map(|row| row.get("name")).collect())
            }
        }
    }

    pub async fn set_download_category(&self, user_id: i64, download_id: i64, category: Option<&str>) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite set_download_category connection")?;
                db::set_download_category(&conn, user_id, download_id, category).context("sqlite set_download_category")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("UPDATE download_history SET category = $3 WHERE id = $1 AND user_id = $2")
                    .bind(download_id)
                    .bind(user_id)
                    .bind(category)
                    .execute(pg_pool)
                    .await
                    .context("postgres set_download_category")?;
                Ok(())
            }
        }
    }

    pub async fn get_cuts_history_filtered(
        &self,
        user_id: i64,
        search_text: Option<&str>,
    ) -> Result<Vec<DownloadHistoryEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_cuts_history_filtered connection")?;
                db::get_cuts_history_filtered(&conn, user_id, search_text).context("sqlite get_cuts_history_filtered")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, original_url AS url, title, output_kind AS format, created_at::text AS downloaded_at,
                            file_id, NULL::text AS author, file_size, duration, video_quality,
                            NULL::text AS audio_bitrate, NULL::text AS bot_api_url, 0::bigint AS bot_api_is_local,
                            source_id, NULL::integer AS part_index, NULL::text AS category, NULL::real AS speed
                     FROM cuts
                     WHERE user_id = $1
                       AND ($2::text IS NULL OR title ILIKE $2)
                     ORDER BY created_at DESC",
                )
                .bind(user_id)
                .bind(search_text.map(|s| format!("%{}%", s)))
                .fetch_all(pg_pool)
                .await
                .context("postgres get_cuts_history_filtered")?;
                Ok(rows.into_iter().map(map_pg_download_history).collect())
            }
        }
    }

    pub async fn save_video_timestamps(&self, download_id: i64, timestamps: &[VideoTimestamp]) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite save_video_timestamps connection")?;
                db::save_video_timestamps(&conn, download_id, timestamps).context("sqlite save_video_timestamps")
            }
            Self::Postgres { pg_pool, .. } => {
                for ts in timestamps {
                    sqlx::query(
                        "INSERT INTO video_timestamps (download_id, source, time_seconds, end_seconds, label)
                         VALUES ($1, $2, $3, $4, $5)",
                    )
                    .bind(download_id)
                    .bind(ts.source.as_str())
                    .bind(ts.time_seconds)
                    .bind(ts.end_seconds)
                    .bind(ts.label.as_deref())
                    .execute(pg_pool)
                    .await
                    .context("postgres save_video_timestamps")?;
                }
                Ok(())
            }
        }
    }

    pub async fn get_video_timestamps(&self, download_id: i64) -> Result<Vec<VideoTimestamp>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_video_timestamps connection")?;
                db::get_video_timestamps(&conn, download_id).context("sqlite get_video_timestamps")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT source, time_seconds, end_seconds, label
                     FROM video_timestamps
                     WHERE download_id = $1
                     ORDER BY time_seconds ASC",
                )
                .bind(download_id)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_video_timestamps")?;
                Ok(rows
                    .into_iter()
                    .map(|row| VideoTimestamp {
                        source: TimestampSource::parse(&row.get::<String, _>("source")),
                        time_seconds: row.get("time_seconds"),
                        end_seconds: row.get("end_seconds"),
                        label: row.get("label"),
                    })
                    .collect())
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_cut(
        &self,
        user_id: i64,
        original_url: &str,
        source_kind: &str,
        source_id: i64,
        output_kind: &str,
        segments_json: &str,
        segments_text: &str,
        title: &str,
        file_id: Option<&str>,
        file_size: Option<i64>,
        duration: Option<i64>,
        video_quality: Option<&str>,
    ) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite create_cut connection")?;
                db::create_cut(
                    &conn,
                    user_id,
                    original_url,
                    source_kind,
                    source_id,
                    output_kind,
                    segments_json,
                    segments_text,
                    title,
                    file_id,
                    file_size,
                    duration,
                    video_quality,
                )
                .context("sqlite create_cut")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "INSERT INTO cuts (
                        user_id, original_url, source_kind, source_id, output_kind, segments_json,
                        segments_text, title, file_id, file_size, duration, video_quality
                     ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
                     RETURNING id",
                )
                .bind(user_id)
                .bind(original_url)
                .bind(source_kind)
                .bind(source_id)
                .bind(output_kind)
                .bind(segments_json)
                .bind(segments_text)
                .bind(title)
                .bind(file_id)
                .bind(file_size)
                .bind(duration)
                .bind(video_quality)
                .fetch_one(pg_pool)
                .await
                .context("postgres create_cut")?;
                Ok(row.get("id"))
            }
        }
    }

    pub async fn update_cut_message_id(&self, cut_id: i64, message_id: i32, chat_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite update_cut_message_id connection")?;
                db::update_cut_message_id(&conn, cut_id, message_id, chat_id).context("sqlite update_cut_message_id")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("UPDATE cuts SET message_id = $2, chat_id = $3 WHERE id = $1")
                    .bind(cut_id)
                    .bind(message_id)
                    .bind(chat_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres update_cut_message_id")?;
                Ok(())
            }
        }
    }

    pub async fn get_cut_message_info(&self, cut_id: i64) -> Result<Option<(i32, i64)>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_cut_message_info connection")?;
                db::get_cut_message_info(&conn, cut_id).context("sqlite get_cut_message_info")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT message_id, chat_id FROM cuts
                     WHERE id = $1 AND message_id IS NOT NULL AND chat_id IS NOT NULL",
                )
                .bind(cut_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_cut_message_info")?;
                Ok(row.map(|row| (row.get("message_id"), row.get("chat_id"))))
            }
        }
    }

    pub async fn get_cuts_count(&self, user_id: i64) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_cuts_count connection")?;
                db::get_cuts_count(&conn, user_id).context("sqlite get_cuts_count")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query("SELECT COUNT(*) AS count FROM cuts WHERE user_id = $1")
                    .bind(user_id)
                    .fetch_one(pg_pool)
                    .await
                    .context("postgres get_cuts_count")?;
                Ok(row.get("count"))
            }
        }
    }

    pub async fn get_cuts_page(&self, user_id: i64, limit: i64, offset: i64) -> Result<Vec<CutEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_cuts_page connection")?;
                db::get_cuts_page(&conn, user_id, limit, offset).context("sqlite get_cuts_page")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, user_id, original_url, source_kind, source_id, output_kind, segments_json,
                            segments_text, title, created_at::text AS created_at, file_id, file_size, duration,
                            video_quality
                     FROM cuts
                     WHERE user_id = $1
                     ORDER BY created_at DESC
                     LIMIT $2 OFFSET $3",
                )
                .bind(user_id)
                .bind(limit)
                .bind(offset)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_cuts_page")?;
                Ok(rows.into_iter().map(map_pg_cut).collect())
            }
        }
    }

    pub async fn get_cut_entry(&self, user_id: i64, cut_id: i64) -> Result<Option<CutEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_cut_entry connection")?;
                db::get_cut_entry(&conn, user_id, cut_id).context("sqlite get_cut_entry")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT id, user_id, original_url, source_kind, source_id, output_kind, segments_json,
                            segments_text, title, created_at::text AS created_at, file_id, file_size, duration,
                            video_quality
                     FROM cuts
                     WHERE id = $1 AND user_id = $2",
                )
                .bind(cut_id)
                .bind(user_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_cut_entry")?;
                Ok(row.map(map_pg_cut))
            }
        }
    }
}

pub(super) fn map_pg_download_history(row: sqlx::postgres::PgRow) -> DownloadHistoryEntry {
    DownloadHistoryEntry {
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
        // Use try_get for backward compatibility: older deployments may not have
        // the `speed` column yet at runtime.
        speed: row.try_get::<Option<f32>, _>("speed").ok().flatten(),
    }
}

fn map_pg_cut(row: sqlx::postgres::PgRow) -> CutEntry {
    use crate::storage::db::{OutputKind, SourceKind};
    CutEntry {
        id: row.get("id"),
        user_id: row.get("user_id"),
        original_url: row.get("original_url"),
        source_kind: SourceKind::from_str_lossy(&row.get::<String, _>("source_kind")),
        source_id: row.get("source_id"),
        output_kind: OutputKind::from_str_lossy(&row.get::<String, _>("output_kind")),
        segments_json: row.get("segments_json"),
        segments_text: row.get("segments_text"),
        title: row.get("title"),
        created_at: row.get("created_at"),
        file_id: row.get("file_id"),
        file_size: row.get("file_size"),
        duration: row.get("duration"),
        video_quality: row.get("video_quality"),
    }
}
