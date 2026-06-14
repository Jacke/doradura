//! `SharedStorage` dispatch for the V48 `popular_files` cache. SQLite branch
//! delegates to `storage/db/popular_files.rs`; Postgres is inline `ON CONFLICT
//! DO UPDATE` with the same semantics.

use anyhow::{Context, Result};
use sqlx::Row;

use crate::storage::db::{self, PopularFileEntry};

use super::SharedStorage;

impl SharedStorage {
    /// Insert or bump a (url, format) cache row. Returns updated hit count.
    ///
    /// Called on every successful download via the alpha.29 hook in
    /// `dorabot::download::pipeline::save_to_history_and_cache`. Cheap
    /// (single row INSERT…ON CONFLICT); safe to call after every send.
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_popular_file(
        &self,
        url: &str,
        format: &str,
        file_id: &str,
        title: Option<&str>,
        author: Option<&str>,
        duration: Option<i64>,
        file_size: Option<i64>,
    ) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite upsert_popular_file connection")?;
                db::upsert_popular_file(&conn, url, format, file_id, title, author, duration, file_size)
                    .context("sqlite upsert_popular_file")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "INSERT INTO popular_files (url, format, file_id, title, author, duration, file_size, hits)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, 1)
                     ON CONFLICT (url, format) DO UPDATE SET
                         file_id    = EXCLUDED.file_id,
                         title      = COALESCE(EXCLUDED.title, popular_files.title),
                         author     = COALESCE(EXCLUDED.author, popular_files.author),
                         duration   = COALESCE(EXCLUDED.duration, popular_files.duration),
                         file_size  = COALESCE(EXCLUDED.file_size, popular_files.file_size),
                         hits       = popular_files.hits + 1,
                         last_used  = NOW()
                     RETURNING hits",
                )
                .bind(url)
                .bind(format)
                .bind(file_id)
                .bind(title)
                .bind(author)
                .bind(duration)
                .bind(file_size)
                .fetch_one(pg_pool)
                .await
                .context("postgres upsert_popular_file")?;
                let hits: i32 = row.get("hits");
                Ok(hits as i64)
            }
        }
    }

    /// Look up a (url, format) cache row. `None` if nobody has downloaded
    /// this combination before. Used by the Guest Bots Path C handler.
    pub async fn lookup_popular_file(&self, url: &str, format: &str) -> Result<Option<PopularFileEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite lookup_popular_file connection")?;
                db::lookup_popular_file(&conn, url, format).context("sqlite lookup_popular_file")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT url, format, file_id, title, author, duration, file_size, hits
                     FROM popular_files
                     WHERE url = $1 AND format = $2",
                )
                .bind(url)
                .bind(format)
                .fetch_optional(pg_pool)
                .await
                .context("postgres lookup_popular_file")?;
                Ok(row.map(|row| PopularFileEntry {
                    url: row.get("url"),
                    format: row.get("format"),
                    file_id: row.get("file_id"),
                    title: row.get("title"),
                    author: row.get("author"),
                    duration: row.get::<Option<i32>, _>("duration").map(|v| v as i64),
                    file_size: row.get("file_size"),
                    hits: row.get::<i32, _>("hits") as i64,
                }))
            }
        }
    }

    /// Look up every cached format for a given URL, ordered by hits DESC.
    /// Drives the inline-mode multi-format URL response: one call returns
    /// every variant (mp3, mp4, m4r, video_note, gif, cut) the bot has seen
    /// for this URL so the user can pick any with a single tap.
    pub async fn lookup_popular_file_all_formats(&self, url: &str) -> Result<Vec<PopularFileEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite lookup_popular_file_all_formats connection")?;
                db::lookup_popular_file_all_formats(&conn, url).context("sqlite lookup_popular_file_all_formats")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT url, format, file_id, title, author, duration, file_size, hits
                     FROM popular_files
                     WHERE url = $1
                     ORDER BY hits DESC",
                )
                .bind(url)
                .fetch_all(pg_pool)
                .await
                .context("postgres lookup_popular_file_all_formats")?;
                Ok(rows
                    .into_iter()
                    .map(|row| PopularFileEntry {
                        url: row.get("url"),
                        format: row.get("format"),
                        file_id: row.get("file_id"),
                        title: row.get("title"),
                        author: row.get("author"),
                        duration: row.get::<Option<i32>, _>("duration").map(|v| v as i64),
                        file_size: row.get("file_size"),
                        hits: row.get::<i32, _>("hits") as i64,
                    })
                    .collect())
            }
        }
    }

    /// Top `limit` globally-most-downloaded files by hit count — Explore Trending.
    pub async fn top_popular_files(&self, limit: u32) -> Result<Vec<PopularFileEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite top_popular_files connection")?;
                db::top_popular_files(&conn, limit).context("sqlite top_popular_files")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT url, format, file_id, title, author, duration, file_size, hits
                     FROM popular_files
                     ORDER BY hits DESC, last_used DESC
                     LIMIT $1",
                )
                .bind(limit as i64)
                .fetch_all(pg_pool)
                .await
                .context("postgres top_popular_files")?;
                Ok(rows
                    .into_iter()
                    .map(|row| PopularFileEntry {
                        url: row.get("url"),
                        format: row.get("format"),
                        file_id: row.get("file_id"),
                        title: row.get("title"),
                        author: row.get("author"),
                        duration: row.get::<Option<i32>, _>("duration").map(|v| v as i64),
                        file_size: row.get("file_size"),
                        hits: row.get::<i32, _>("hits") as i64,
                    })
                    .collect())
            }
        }
    }

    /// Look up the most recent `download_history` entry for a (telegram_id, url, format)
    /// triple — drives the Guest Bots Path A "user already had this file" branch.
    ///
    /// Returns `(file_id, title, author, duration, file_size)` so the caller
    /// can stitch together an `InlineQueryResultCached*` reply directly.
    pub async fn lookup_personal_file(
        &self,
        telegram_id: i64,
        url: &str,
        format: &str,
    ) -> Result<Option<(String, Option<String>, Option<String>, Option<i64>, Option<i64>)>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite lookup_personal_file connection")?;
                let mut stmt = conn.prepare(
                    "SELECT file_id, title, author, duration, file_size
                     FROM download_history
                     WHERE user_id = ?1 AND url = ?2 AND format = ?3 AND file_id IS NOT NULL
                     ORDER BY downloaded_at DESC LIMIT 1",
                )?;
                let row = stmt
                    .query_map(rusqlite::params![telegram_id, url, format], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<i64>>(3)?,
                            row.get::<_, Option<i64>>(4)?,
                        ))
                    })?
                    .next()
                    .transpose()?;
                Ok(row)
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT file_id, title, author, duration, file_size
                     FROM download_history
                     WHERE user_id = $1 AND url = $2 AND format = $3 AND file_id IS NOT NULL
                     ORDER BY downloaded_at DESC LIMIT 1",
                )
                .bind(telegram_id)
                .bind(url)
                .bind(format)
                .fetch_optional(pg_pool)
                .await
                .context("postgres lookup_personal_file")?;
                Ok(row.map(|row| {
                    (
                        row.get("file_id"),
                        row.get("title"),
                        row.get("author"),
                        row.get("duration"),
                        row.get("file_size"),
                    )
                }))
            }
        }
    }
}
