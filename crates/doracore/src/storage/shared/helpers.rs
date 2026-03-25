use anyhow::{Context, Result};
use sqlx::Row;

use crate::storage::db::{self, DbConnection};

use super::SharedStorage;

impl SharedStorage {
    pub async fn register_processed_update(&self, bot_id: i64, update_id: i64) -> Result<bool> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite register_processed_update connection")?;
                db::register_processed_update(&conn, bot_id, update_id).context("sqlite register_processed_update")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "INSERT INTO processed_updates (bot_id, update_id) VALUES ($1, $2)
                     ON CONFLICT DO NOTHING",
                )
                .bind(bot_id)
                .bind(update_id)
                .execute(pg_pool)
                .await
                .context("postgres register_processed_update")?
                .rows_affected();
                Ok(rows > 0)
            }
        }
    }

    pub async fn cleanup_old_processed_updates(&self, hours: i64) -> Result<u64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite cleanup_old_processed_updates connection")?;
                Ok(
                    db::cleanup_old_processed_updates(&conn, hours).context("sqlite cleanup_old_processed_updates")?
                        as u64,
                )
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "DELETE FROM processed_updates WHERE created_at < NOW() - ($1::bigint * INTERVAL '1 hour')",
                )
                .bind(hours)
                .execute(pg_pool)
                .await
                .context("postgres cleanup_old_processed_updates")?
                .rows_affected();
                Ok(rows)
            }
        }
    }

    // Private helpers below — used by user_settings.rs and sessions.rs

    pub(super) async fn get_user_string_setting(
        &self,
        telegram_id: i64,
        sqlite_selector: &str,
        postgres_query: &str,
        default_value: &str,
    ) -> Result<String> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user_string_setting connection")?;
                match sqlite_selector {
                    "language" => db::get_user_language(&conn, telegram_id),
                    "progress_bar_style" => db::get_user_progress_bar_style(&conn, telegram_id),
                    "video_quality" => db::get_user_video_quality(&conn, telegram_id),
                    "audio_bitrate" => db::get_user_audio_bitrate(&conn, telegram_id),
                    _ => Err(rusqlite::Error::InvalidQuery),
                }
                .map_err(anyhow::Error::from)
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(postgres_query)
                    .bind(telegram_id)
                    .fetch_optional(pg_pool)
                    .await
                    .context("postgres get_user_string_setting")?;
                Ok(row
                    .map(|row| row.get::<String, _>(sqlite_selector))
                    .unwrap_or_else(|| default_value.to_string()))
            }
        }
    }

    pub(super) async fn get_user_i32_setting(
        &self,
        telegram_id: i64,
        sqlite_selector: &str,
        postgres_query: &str,
        default_value: i32,
    ) -> Result<i32> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user_i32_setting connection")?;
                match sqlite_selector {
                    "send_as_document" => db::get_user_send_as_document(&conn, telegram_id),
                    "send_audio_as_document" => db::get_user_send_audio_as_document(&conn, telegram_id),
                    "download_subtitles" => {
                        db::get_user_download_subtitles(&conn, telegram_id).map(|value| value as i32)
                    }
                    "burn_subtitles" => db::get_user_burn_subtitles(&conn, telegram_id).map(|value| value as i32),
                    "experimental_features" => {
                        db::get_user_experimental_features(&conn, telegram_id).map(|value| value as i32)
                    }
                    _ => Err(rusqlite::Error::InvalidQuery),
                }
                .map_err(anyhow::Error::from)
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(postgres_query)
                    .bind(telegram_id)
                    .fetch_optional(pg_pool)
                    .await
                    .context("postgres get_user_i32_setting")?;
                Ok(row
                    .map(|row| row.get::<i32, _>(sqlite_selector))
                    .unwrap_or(default_value))
            }
        }
    }

    pub(super) async fn set_user_string_setting(
        &self,
        telegram_id: i64,
        sqlite_selector: &str,
        value: &str,
        postgres_query: &str,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite set_user_string_setting connection")?;
                match sqlite_selector {
                    "language" => db::set_user_language(&conn, telegram_id, value),
                    "progress_bar_style" => db::set_user_progress_bar_style(&conn, telegram_id, value),
                    "video_quality" => db::set_user_video_quality(&conn, telegram_id, value),
                    "audio_bitrate" => db::set_user_audio_bitrate(&conn, telegram_id, value),
                    "subtitle_font_size" => db::set_user_subtitle_font_size(&conn, telegram_id, value),
                    "subtitle_text_color" => db::set_user_subtitle_text_color(&conn, telegram_id, value),
                    "subtitle_outline_color" => db::set_user_subtitle_outline_color(&conn, telegram_id, value),
                    "subtitle_position" => db::set_user_subtitle_position(&conn, telegram_id, value),
                    _ => Err(rusqlite::Error::InvalidQuery),
                }
                .map_err(anyhow::Error::from)
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(postgres_query)
                    .bind(telegram_id)
                    .bind(value)
                    .execute(pg_pool)
                    .await
                    .context("postgres set_user_string_setting")?;
                Ok(())
            }
        }
    }

    pub(super) async fn set_user_i32_setting(
        &self,
        telegram_id: i64,
        sqlite_selector: &str,
        value: i32,
        postgres_query: &str,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite set_user_i32_setting connection")?;
                match sqlite_selector {
                    "send_as_document" => db::set_user_send_as_document(&conn, telegram_id, value),
                    "send_audio_as_document" => db::set_user_send_audio_as_document(&conn, telegram_id, value),
                    "burn_subtitles" => db::set_user_burn_subtitles(&conn, telegram_id, value != 0),
                    "subtitle_outline_width" => db::set_user_subtitle_outline_width(&conn, telegram_id, value),
                    "subtitle_shadow" => db::set_user_subtitle_shadow(&conn, telegram_id, value),
                    "experimental_features" => db::set_user_experimental_features(&conn, telegram_id, value != 0),
                    _ => Err(rusqlite::Error::InvalidQuery),
                }
                .map_err(anyhow::Error::from)
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(postgres_query)
                    .bind(telegram_id)
                    .bind(value)
                    .execute(pg_pool)
                    .await
                    .context("postgres set_user_i32_setting")?;
                Ok(())
            }
        }
    }

    pub(super) async fn delete_session_by_user<F>(
        &self,
        user_id: i64,
        table_name: &'static str,
        sqlite_context: &'static str,
        sqlite_delete: F,
    ) -> Result<()>
    where
        F: FnOnce(&DbConnection, i64) -> rusqlite::Result<()>,
    {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context(sqlite_context)?;
                sqlite_delete(&conn, user_id).map_err(anyhow::Error::from)
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(&format!("DELETE FROM {table_name} WHERE user_id = $1"))
                    .bind(user_id)
                    .execute(pg_pool)
                    .await
                    .with_context(|| format!("postgres delete from {table_name}"))?;
                Ok(())
            }
        }
    }
}
