use anyhow::{Context, Result};
use sqlx::Row;

use crate::storage::db::{self, ErrorLogEntry};

use super::SharedStorage;

impl SharedStorage {
    pub async fn log_alert_history(
        &self,
        alert_type: &str,
        severity: &str,
        message: &str,
        metadata: Option<&str>,
        triggered_at: &str,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite log_alert_history connection")?;
                conn.execute(
                    "INSERT INTO alert_history (alert_type, severity, message, metadata, triggered_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![alert_type, severity, message, metadata, triggered_at],
                )
                .context("sqlite log_alert_history")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO alert_history (alert_type, severity, message, metadata, triggered_at)
                     VALUES ($1, $2, $3, $4, $5::timestamptz)",
                )
                .bind(alert_type)
                .bind(severity)
                .bind(message)
                .bind(metadata)
                .bind(triggered_at)
                .execute(pg_pool)
                .await
                .context("postgres log_alert_history")?;
                Ok(())
            }
        }
    }

    pub async fn resolve_alert_history(&self, alert_type: &str, resolved_at: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite resolve_alert_history connection")?;
                conn.execute(
                    "UPDATE alert_history SET resolved_at = ?1 WHERE alert_type = ?2 AND resolved_at IS NULL",
                    rusqlite::params![resolved_at, alert_type],
                )
                .context("sqlite resolve_alert_history")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE alert_history SET resolved_at = $1::timestamptz
                     WHERE alert_type = $2 AND resolved_at IS NULL",
                )
                .bind(resolved_at)
                .bind(alert_type)
                .execute(pg_pool)
                .await
                .context("postgres resolve_alert_history")?;
                Ok(())
            }
        }
    }

    pub async fn log_error(
        &self,
        user_id: Option<i64>,
        username: Option<&str>,
        error_type: &str,
        error_message: &str,
        url: Option<&str>,
        context: Option<&str>,
    ) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite log_error connection")?;
                db::log_error(&conn, user_id, username, error_type, error_message, url, context)
                    .context("sqlite log_error")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "INSERT INTO error_log (user_id, username, error_type, error_message, url, context)
                     VALUES ($1, $2, $3, $4, $5, $6)
                     RETURNING id",
                )
                .bind(user_id)
                .bind(username)
                .bind(error_type)
                .bind(error_message)
                .bind(url)
                .bind(context)
                .fetch_one(pg_pool)
                .await
                .context("postgres log_error")?;
                Ok(row.get("id"))
            }
        }
    }

    pub async fn get_recent_errors(&self, hours: i64, limit: i64) -> Result<Vec<ErrorLogEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_recent_errors connection")?;
                db::get_recent_errors(&conn, hours, limit).context("sqlite get_recent_errors")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, timestamp::text AS timestamp, user_id, username, error_type, error_message, url, context, resolved
                     FROM error_log
                     WHERE timestamp >= NOW() - make_interval(hours => $1)
                     ORDER BY timestamp DESC
                     LIMIT $2",
                )
                .bind(hours as i32)
                .bind(limit)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_recent_errors")?;
                Ok(rows.into_iter().map(map_pg_error_log_entry).collect())
            }
        }
    }

    pub async fn get_error_stats(&self, hours: i64) -> Result<Vec<(String, i64)>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_error_stats connection")?;
                db::get_error_stats(&conn, hours).context("sqlite get_error_stats")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT error_type, COUNT(*)::bigint AS cnt
                     FROM error_log
                     WHERE timestamp >= NOW() - make_interval(hours => $1)
                     GROUP BY error_type
                     ORDER BY cnt DESC",
                )
                .bind(hours as i32)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_error_stats")?;
                Ok(rows
                    .into_iter()
                    .map(|row| (row.get::<String, _>("error_type"), row.get::<i64, _>("cnt")))
                    .collect())
            }
        }
    }

    pub async fn cleanup_old_errors(&self, days: i64) -> Result<usize> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite cleanup_old_errors connection")?;
                db::cleanup_old_errors(&conn, days).context("sqlite cleanup_old_errors")
            }
            Self::Postgres { pg_pool, .. } => {
                let result = sqlx::query("DELETE FROM error_log WHERE timestamp < NOW() - make_interval(days => $1)")
                    .bind(days as i32)
                    .execute(pg_pool)
                    .await
                    .context("postgres cleanup_old_errors")?;
                Ok(result.rows_affected() as usize)
            }
        }
    }

    pub async fn cleanup_old_tasks(&self, days: i64) -> Result<usize> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite cleanup_old_tasks connection")?;
                db::cleanup_old_tasks(&conn, days).context("sqlite cleanup_old_tasks")
            }
            Self::Postgres { pg_pool, .. } => {
                let result = sqlx::query(
                    "DELETE FROM task_queue
                     WHERE status IN ('completed', 'dead_letter')
                       AND updated_at < NOW() - make_interval(days => $1)",
                )
                .bind(days as i32)
                .execute(pg_pool)
                .await
                .context("postgres cleanup_old_tasks")?;
                Ok(result.rows_affected() as usize)
            }
        }
    }
}

fn map_pg_error_log_entry(row: sqlx::postgres::PgRow) -> ErrorLogEntry {
    ErrorLogEntry {
        id: row.get("id"),
        timestamp: row.get("timestamp"),
        user_id: row.get("user_id"),
        username: row.get("username"),
        error_type: row.get("error_type"),
        error_message: row.get("error_message"),
        url: row.get("url"),
        context: row.get("context"),
        resolved: row.get::<i32, _>("resolved") != 0,
    }
}
