use anyhow::{Context, Result};
use sqlx::Row;

use crate::storage::db;
use crate::storage::uploads::{self, NewUpload, UploadEntry};

use super::SharedStorage;

impl SharedStorage {
    pub async fn save_upload(&self, upload: &NewUpload<'_>) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite save_upload connection")?;
                uploads::save_upload(&conn, upload)
                    .map_err(anyhow::Error::from)
                    .context("sqlite save_upload")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "INSERT INTO uploads (
                        user_id, original_filename, title, media_type, file_format,
                        file_id, file_unique_id, file_size, duration, width, height,
                        mime_type, message_id, chat_id, thumbnail_file_id
                     ) VALUES (
                        $1, $2, $3, $4, $5,
                        $6, $7, $8, $9, $10, $11,
                        $12, $13, $14, $15
                     )
                     RETURNING id",
                )
                .bind(upload.user_id)
                .bind(upload.original_filename)
                .bind(upload.title)
                .bind(upload.media_type)
                .bind(upload.file_format)
                .bind(upload.file_id)
                .bind(upload.file_unique_id)
                .bind(upload.file_size)
                .bind(upload.duration)
                .bind(upload.width)
                .bind(upload.height)
                .bind(upload.mime_type)
                .bind(upload.message_id)
                .bind(upload.chat_id)
                .bind(upload.thumbnail_file_id)
                .fetch_one(pg_pool)
                .await
                .context("postgres save_upload")?;
                Ok(row.get("id"))
            }
        }
    }

    pub async fn find_duplicate_upload(&self, user_id: i64, file_unique_id: &str) -> Result<Option<UploadEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite find_duplicate_upload connection")?;
                uploads::find_duplicate_upload(&conn, user_id, file_unique_id)
                    .map_err(anyhow::Error::from)
                    .context("sqlite find_duplicate_upload")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT
                        id, user_id, original_filename, title, media_type, file_format,
                        file_id, file_unique_id, file_size, duration, width, height,
                        mime_type, message_id, chat_id, CAST(uploaded_at AS TEXT) AS uploaded_at, thumbnail_file_id
                     FROM uploads
                     WHERE user_id = $1 AND file_unique_id = $2
                     ORDER BY uploaded_at DESC
                     LIMIT 1",
                )
                .bind(user_id)
                .bind(file_unique_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres find_duplicate_upload")?;
                Ok(row.as_ref().map(upload_entry_from_pg_row))
            }
        }
    }

    pub async fn get_upload_by_id(&self, user_id: i64, upload_id: i64) -> Result<Option<UploadEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_upload_by_id connection")?;
                uploads::get_upload_by_id(&conn, user_id, upload_id)
                    .map_err(anyhow::Error::from)
                    .context("sqlite get_upload_by_id")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT
                        id, user_id, original_filename, title, media_type, file_format,
                        file_id, file_unique_id, file_size, duration, width, height,
                        mime_type, message_id, chat_id, CAST(uploaded_at AS TEXT) AS uploaded_at, thumbnail_file_id
                     FROM uploads
                     WHERE id = $1 AND user_id = $2",
                )
                .bind(upload_id)
                .bind(user_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_upload_by_id")?;
                Ok(row.as_ref().map(upload_entry_from_pg_row))
            }
        }
    }

    pub async fn get_uploads_filtered(
        &self,
        user_id: i64,
        media_type_filter: Option<&str>,
        search_text: Option<&str>,
    ) -> Result<Vec<UploadEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_uploads_filtered connection")?;
                uploads::get_uploads_filtered(&conn, user_id, media_type_filter, search_text)
                    .map_err(anyhow::Error::from)
                    .context("sqlite get_uploads_filtered")
            }
            Self::Postgres { pg_pool, .. } => {
                let base = "SELECT
                        id, user_id, original_filename, title, media_type, file_format,
                        file_id, file_unique_id, file_size, duration, width, height,
                        mime_type, message_id, chat_id, CAST(uploaded_at AS TEXT) AS uploaded_at, thumbnail_file_id
                     FROM uploads
                     WHERE user_id = $1";
                let rows = match (media_type_filter, search_text) {
                    (Some(media_type), Some(search)) => {
                        sqlx::query(&format!(
                            "{base} AND media_type = $2 AND title ILIKE $3 ORDER BY uploaded_at DESC"
                        ))
                        .bind(user_id)
                        .bind(media_type)
                        .bind(format!("%{}%", search))
                        .fetch_all(pg_pool)
                        .await
                    }
                    (Some(media_type), None) => {
                        sqlx::query(&format!("{base} AND media_type = $2 ORDER BY uploaded_at DESC"))
                            .bind(user_id)
                            .bind(media_type)
                            .fetch_all(pg_pool)
                            .await
                    }
                    (None, Some(search)) => {
                        sqlx::query(&format!("{base} AND title ILIKE $2 ORDER BY uploaded_at DESC"))
                            .bind(user_id)
                            .bind(format!("%{}%", search))
                            .fetch_all(pg_pool)
                            .await
                    }
                    (None, None) => {
                        sqlx::query(&format!("{base} ORDER BY uploaded_at DESC"))
                            .bind(user_id)
                            .fetch_all(pg_pool)
                            .await
                    }
                }
                .context("postgres get_uploads_filtered")?;
                Ok(rows.iter().map(upload_entry_from_pg_row).collect())
            }
        }
    }

    pub async fn delete_upload(&self, user_id: i64, upload_id: i64) -> Result<bool> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite delete_upload connection")?;
                uploads::delete_upload(&conn, user_id, upload_id)
                    .map_err(anyhow::Error::from)
                    .context("sqlite delete_upload")
            }
            Self::Postgres { pg_pool, .. } => {
                let result = sqlx::query("DELETE FROM uploads WHERE id = $1 AND user_id = $2")
                    .bind(upload_id)
                    .bind(user_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres delete_upload")?;
                Ok(result.rows_affected() > 0)
            }
        }
    }
}

fn upload_entry_from_pg_row(row: &sqlx::postgres::PgRow) -> UploadEntry {
    UploadEntry {
        id: row.get("id"),
        user_id: row.get("user_id"),
        original_filename: row.get("original_filename"),
        title: row.get("title"),
        media_type: row.get("media_type"),
        file_format: row.get("file_format"),
        file_id: row.get("file_id"),
        file_unique_id: row.get("file_unique_id"),
        file_size: row.get("file_size"),
        duration: row.get("duration"),
        width: row.get("width"),
        height: row.get("height"),
        mime_type: row.get("mime_type"),
        message_id: row.get("message_id"),
        chat_id: row.get("chat_id"),
        uploaded_at: row.get("uploaded_at"),
        thumbnail_file_id: row.get("thumbnail_file_id"),
    }
}
