use anyhow::{Context, Result};
use sqlx::Row;

use crate::storage::db::{self, UserVault};

use super::SharedStorage;

impl SharedStorage {
    pub async fn get_user_vault(&self, user_id: i64) -> Result<Option<UserVault>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user_vault connection")?;
                db::get_user_vault(&conn, user_id).context("sqlite get_user_vault")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT user_id, channel_id, channel_title, is_active, created_at::text AS created_at
                     FROM user_vaults
                     WHERE user_id = $1",
                )
                .bind(user_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_user_vault")?;
                Ok(row.map(|row| UserVault {
                    user_id: row.get("user_id"),
                    channel_id: row.get("channel_id"),
                    channel_title: row.get("channel_title"),
                    is_active: row.get::<i32, _>("is_active") != 0,
                    created_at: row.get("created_at"),
                }))
            }
        }
    }

    pub async fn set_user_vault(&self, user_id: i64, channel_id: i64, channel_title: Option<&str>) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite set_user_vault connection")?;
                db::set_user_vault(&conn, user_id, channel_id, channel_title).context("sqlite set_user_vault")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO user_vaults (user_id, channel_id, channel_title, is_active, created_at, updated_at)
                     VALUES ($1, $2, $3, 1, NOW(), NOW())
                     ON CONFLICT (user_id) DO UPDATE SET
                        channel_id = EXCLUDED.channel_id,
                        channel_title = EXCLUDED.channel_title,
                        is_active = 1,
                        updated_at = NOW()",
                )
                .bind(user_id)
                .bind(channel_id)
                .bind(channel_title)
                .execute(pg_pool)
                .await
                .context("postgres set_user_vault")?;
                Ok(())
            }
        }
    }

    pub async fn deactivate_user_vault(&self, user_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite deactivate_user_vault connection")?;
                db::deactivate_user_vault(&conn, user_id).context("sqlite deactivate_user_vault")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("UPDATE user_vaults SET is_active = 0, updated_at = NOW() WHERE user_id = $1")
                    .bind(user_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres deactivate_user_vault")?;
                Ok(())
            }
        }
    }

    pub async fn activate_user_vault(&self, user_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite activate_user_vault connection")?;
                db::activate_user_vault(&conn, user_id).context("sqlite activate_user_vault")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("UPDATE user_vaults SET is_active = 1, updated_at = NOW() WHERE user_id = $1")
                    .bind(user_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres activate_user_vault")?;
                Ok(())
            }
        }
    }

    pub async fn delete_user_vault(&self, user_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite delete_user_vault connection")?;
                db::delete_user_vault(&conn, user_id).context("sqlite delete_user_vault")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("DELETE FROM user_vaults WHERE user_id = $1")
                    .bind(user_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres delete_user_vault")?;
                Ok(())
            }
        }
    }

    pub async fn get_vault_cached_file_id(&self, user_id: i64, url: &str) -> Result<Option<String>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_vault_cached_file_id connection")?;
                Ok(db::get_vault_cached_file_id(&conn, user_id, url))
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query("SELECT file_id FROM vault_cache WHERE user_id = $1 AND url = $2")
                    .bind(user_id)
                    .bind(url)
                    .fetch_optional(pg_pool)
                    .await
                    .context("postgres get_vault_cached_file_id")?;
                Ok(row.map(|row| row.get("file_id")))
            }
        }
    }

    pub async fn save_vault_cache_entry(
        &self,
        user_id: i64,
        url: &str,
        title: Option<&str>,
        artist: Option<&str>,
        duration_secs: Option<i32>,
        file_id: &str,
        message_id: Option<i64>,
        file_size: Option<i64>,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite save_vault_cache_entry connection")?;
                db::save_vault_cache_entry(
                    &conn,
                    user_id,
                    url,
                    title,
                    artist,
                    duration_secs,
                    file_id,
                    message_id,
                    file_size,
                )
                .context("sqlite save_vault_cache_entry")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO vault_cache (
                        user_id, url, title, artist, duration_secs, file_id, message_id, file_size, created_at
                     ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW())
                     ON CONFLICT (user_id, url) DO UPDATE SET
                        title = EXCLUDED.title,
                        artist = EXCLUDED.artist,
                        duration_secs = EXCLUDED.duration_secs,
                        file_id = EXCLUDED.file_id,
                        message_id = EXCLUDED.message_id,
                        file_size = EXCLUDED.file_size,
                        created_at = NOW()",
                )
                .bind(user_id)
                .bind(url)
                .bind(title)
                .bind(artist)
                .bind(duration_secs)
                .bind(file_id)
                .bind(message_id)
                .bind(file_size)
                .execute(pg_pool)
                .await
                .context("postgres save_vault_cache_entry")?;
                Ok(())
            }
        }
    }

    pub async fn get_vault_cache_stats(&self, user_id: i64) -> Result<(i64, i64)> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_vault_cache_stats connection")?;
                Ok(db::get_vault_cache_stats(&conn, user_id))
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT COUNT(*)::BIGINT AS count, COALESCE(SUM(file_size), 0)::BIGINT AS total_bytes
                     FROM vault_cache
                     WHERE user_id = $1",
                )
                .bind(user_id)
                .fetch_one(pg_pool)
                .await
                .context("postgres get_vault_cache_stats")?;
                Ok((row.get("count"), row.get("total_bytes")))
            }
        }
    }

    pub async fn clear_vault_cache(&self, user_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite clear_vault_cache connection")?;
                db::clear_vault_cache(&conn, user_id).context("sqlite clear_vault_cache")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("DELETE FROM vault_cache WHERE user_id = $1")
                    .bind(user_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres clear_vault_cache")?;
                Ok(())
            }
        }
    }
}
