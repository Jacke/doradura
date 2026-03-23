use anyhow::{Context, Result};
use rusqlite::OptionalExtension;
use sqlx::Row;

use crate::storage::db;

use super::types::SharePageRecord;
use super::SharedStorage;

impl SharedStorage {
    pub async fn create_share_page_record(
        &self,
        id: &str,
        youtube_url: &str,
        title: &str,
        artist: Option<&str>,
        thumbnail_url: Option<&str>,
        duration_secs: Option<i64>,
        streaming_links_json: Option<&str>,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite create_share_page_record connection")?;
                conn.execute(
                    "INSERT INTO share_pages (id, youtube_url, title, artist, thumbnail_url, duration_secs, streaming_links)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        id,
                        youtube_url,
                        title,
                        artist,
                        thumbnail_url,
                        duration_secs,
                        streaming_links_json
                    ],
                )
                .context("sqlite create_share_page_record")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO share_pages (id, youtube_url, title, artist, thumbnail_url, duration_secs, streaming_links)
                     VALUES ($1, $2, $3, $4, $5, $6, $7)",
                )
                .bind(id)
                .bind(youtube_url)
                .bind(title)
                .bind(artist)
                .bind(thumbnail_url)
                .bind(duration_secs)
                .bind(streaming_links_json)
                .execute(pg_pool)
                .await
                .context("postgres create_share_page_record")?;
                Ok(())
            }
        }
    }

    pub async fn get_share_page_record(&self, id: &str) -> Result<Option<SharePageRecord>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_share_page_record connection")?;
                let result = conn
                    .query_row(
                        "SELECT id, youtube_url, title, artist, thumbnail_url, duration_secs, streaming_links, created_at
                         FROM share_pages WHERE id = ?1",
                        rusqlite::params![id],
                        |row| {
                            Ok(SharePageRecord {
                                id: row.get(0)?,
                                youtube_url: row.get(1)?,
                                title: row.get(2)?,
                                artist: row.get(3)?,
                                thumbnail_url: row.get(4)?,
                                duration_secs: row.get(5)?,
                                streaming_links_json: row.get(6)?,
                                created_at: row.get(7)?,
                            })
                        },
                    )
                    .optional()
                    .context("sqlite get_share_page_record")?;
                Ok(result)
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT id, youtube_url, title, artist, thumbnail_url, duration_secs,
                            streaming_links, created_at::text AS created_at
                     FROM share_pages WHERE id = $1",
                )
                .bind(id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_share_page_record")?;
                Ok(row.map(map_pg_share_page_record))
            }
        }
    }

    pub async fn store_cached_url(&self, id: &str, url: &str, expires_at: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite store_cached_url connection")?;
                conn.execute(
                    "INSERT OR REPLACE INTO url_cache (id, url, expires_at) VALUES (?1, ?2, ?3)",
                    rusqlite::params![id, url, expires_at],
                )
                .context("sqlite store_cached_url")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO url_cache (id, url, expires_at)
                     VALUES ($1, $2, $3::timestamptz)
                     ON CONFLICT (id) DO UPDATE
                     SET url = EXCLUDED.url,
                         expires_at = EXCLUDED.expires_at",
                )
                .bind(id)
                .bind(url)
                .bind(expires_at)
                .execute(pg_pool)
                .await
                .context("postgres store_cached_url")?;
                Ok(())
            }
        }
    }

    pub async fn get_cached_url(&self, id: &str) -> Result<Option<String>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_cached_url connection")?;
                let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
                conn.query_row(
                    "SELECT url FROM url_cache WHERE id = ?1 AND expires_at > ?2",
                    rusqlite::params![id, now],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .context("sqlite get_cached_url")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query("SELECT url FROM url_cache WHERE id = $1 AND expires_at > NOW()")
                    .bind(id)
                    .fetch_optional(pg_pool)
                    .await
                    .context("postgres get_cached_url")?;
                Ok(row.map(|row| row.get("url")))
            }
        }
    }

    pub async fn cleanup_expired_url_cache(&self) -> Result<usize> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite cleanup_expired_url_cache connection")?;
                let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
                conn.execute("DELETE FROM url_cache WHERE expires_at <= ?1", rusqlite::params![now])
                    .context("sqlite cleanup_expired_url_cache")
            }
            Self::Postgres { pg_pool, .. } => {
                let result = sqlx::query("DELETE FROM url_cache WHERE expires_at <= NOW()")
                    .execute(pg_pool)
                    .await
                    .context("postgres cleanup_expired_url_cache")?;
                Ok(result.rows_affected() as usize)
            }
        }
    }
}

fn map_pg_share_page_record(row: sqlx::postgres::PgRow) -> SharePageRecord {
    SharePageRecord {
        id: row.get("id"),
        youtube_url: row.get("youtube_url"),
        title: row.get("title"),
        artist: row.get("artist"),
        thumbnail_url: row.get("thumbnail_url"),
        duration_secs: row.get("duration_secs"),
        streaming_links_json: row.get("streaming_links"),
        created_at: row.get("created_at"),
    }
}
