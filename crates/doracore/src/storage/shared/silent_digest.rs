//! `SharedStorage` dispatch for the V49 `silent_digest` table. SQLite branch
//! delegates to `storage/db/silent_digest.rs`; Postgres is inline `UPDATE …
//! RETURNING` with the same atomic fetch-and-mark semantics.

use anyhow::{Context, Result};
use sqlx::Row;

use crate::storage::db::{self, SilentDigestEntry};

use super::SharedStorage;

impl SharedStorage {
    /// Record a completed (or failed) silent download awaiting a MOTD recap.
    /// `status` is `"done"` or `"failed"`.
    pub async fn insert_silent_digest(
        &self,
        user_id: i64,
        title: Option<&str>,
        format: Option<&str>,
        status: &str,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite insert_silent_digest connection")?;
                db::insert_silent_digest(&conn, user_id, title, format, status).context("sqlite insert_silent_digest")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("INSERT INTO silent_digest (user_id, title, format, status) VALUES ($1, $2, $3, $4)")
                    .bind(user_id)
                    .bind(title)
                    .bind(format)
                    .bind(status)
                    .execute(pg_pool)
                    .await
                    .context("postgres insert_silent_digest")?;
                Ok(())
            }
        }
    }

    /// Atomically fetch all unshown digest rows for a user and mark them shown.
    /// Idempotent — a concurrent second call returns an empty Vec, so the MOTD
    /// is never shown twice.
    pub async fn take_unshown_silent_digest(&self, user_id: i64) -> Result<Vec<SilentDigestEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite take_unshown_silent_digest connection")?;
                db::take_unshown_silent_digest(&conn, user_id).context("sqlite take_unshown_silent_digest")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "UPDATE silent_digest SET shown = 1
                     WHERE user_id = $1 AND shown = 0
                     RETURNING title, format, status",
                )
                .bind(user_id)
                .fetch_all(pg_pool)
                .await
                .context("postgres take_unshown_silent_digest")?;
                Ok(rows
                    .into_iter()
                    .map(|row| SilentDigestEntry {
                        title: row.get("title"),
                        format: row.get("format"),
                        status: row.get("status"),
                    })
                    .collect())
            }
        }
    }
}
