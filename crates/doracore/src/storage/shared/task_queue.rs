use anyhow::{Context, Result};
use sqlx::Row;

use crate::storage::db::{self, EnqueueResult, TaskQueueEntry};

use super::SharedStorage;
use super::types::QueueTaskInput;

/// Task status transitions used by `run_task_status_update`.
enum TaskStatusUpdate {
    Processing,
    Uploading,
}

impl SharedStorage {
    pub async fn save_task_to_queue(&self, input: QueueTaskInput<'_>) -> Result<EnqueueResult> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite save_task_to_queue connection")?;
                db::save_task_to_queue(
                    &conn,
                    input.task_id,
                    input.user_id,
                    input.url,
                    input.message_id,
                    input.format,
                    input.is_video,
                    input.video_quality,
                    input.audio_bitrate,
                    input.time_range_start,
                    input.time_range_end,
                    input.carousel_mask,
                    input.priority,
                    input.idempotency_key,
                )
                .context("sqlite save_task_to_queue")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "INSERT INTO task_queue (
                        id, user_id, url, message_id, format, is_video, video_quality, audio_bitrate,
                        time_range_start, time_range_end, carousel_mask, priority, status, retry_count, idempotency_key
                     ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, 'pending', 0, $13)
                     ON CONFLICT DO NOTHING",
                )
                .bind(input.task_id)
                .bind(input.user_id)
                .bind(input.url)
                .bind(input.message_id)
                .bind(input.format)
                .bind(if input.is_video { 1_i32 } else { 0_i32 })
                .bind(input.video_quality)
                .bind(input.audio_bitrate)
                .bind(input.time_range_start)
                .bind(input.time_range_end)
                .bind(input.carousel_mask.map(|value| value as i32))
                .bind(input.priority)
                .bind(input.idempotency_key)
                .execute(pg_pool)
                .await
                .context("postgres save_task_to_queue")?
                .rows_affected();
                Ok(if rows == 0 {
                    EnqueueResult::Duplicate
                } else {
                    EnqueueResult::Enqueued
                })
            }
        }
    }

    pub async fn claim_next_task(&self, worker_id: &str, lease_seconds: i64) -> Result<Option<TaskQueueEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite claim_next_task connection")?;
                db::claim_next_task(&conn, worker_id, lease_seconds).context("sqlite claim_next_task")
            }
            Self::Postgres { pg_pool, .. } => {
                let mut tx = pg_pool.begin().await.context("postgres claim_next_task begin")?;
                let row = sqlx::query(
                    "WITH candidate AS (
                        SELECT id
                        FROM task_queue
                        WHERE status = 'pending'
                          AND (execute_at IS NULL OR execute_at <= NOW())
                        ORDER BY priority DESC, created_at ASC
                        FOR UPDATE SKIP LOCKED
                        LIMIT 1
                    )
                    UPDATE task_queue t
                    SET status = 'leased',
                        worker_id = $1,
                        leased_at = NOW(),
                        lease_expires_at = NOW() + ($2::bigint * INTERVAL '1 second'),
                        last_heartbeat_at = NOW(),
                        updated_at = NOW()
                    FROM candidate
                    WHERE t.id = candidate.id
                    RETURNING t.*",
                )
                .bind(worker_id)
                .bind(lease_seconds)
                .fetch_optional(&mut *tx)
                .await
                .context("postgres claim_next_task update")?;
                tx.commit().await.context("postgres claim_next_task commit")?;
                row.map(map_pg_task_queue_entry).transpose()
            }
        }
    }

    pub async fn heartbeat_worker_leases(&self, worker_id: &str, lease_seconds: i64) -> Result<u64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite heartbeat_worker_leases connection")?;
                Ok(db::heartbeat_worker_leases(&conn, worker_id, lease_seconds)
                    .context("sqlite heartbeat_worker_leases")? as u64)
            }
            Self::Postgres { pg_pool, .. } => Ok(sqlx::query(
                "UPDATE task_queue
                 SET lease_expires_at = NOW() + ($1::bigint * INTERVAL '1 second'),
                     last_heartbeat_at = NOW(),
                     updated_at = NOW()
                 WHERE worker_id = $2
                   AND status IN ('leased', 'processing', 'uploading')",
            )
            .bind(lease_seconds)
            .bind(worker_id)
            .execute(pg_pool)
            .await
            .context("postgres heartbeat_worker_leases")?
            .rows_affected()),
        }
    }

    pub async fn mark_task_processing(&self, task_id: &str, worker_id: &str) -> Result<()> {
        self.run_task_status_update(TaskStatusUpdate::Processing, task_id, worker_id)
            .await
    }

    pub async fn mark_task_uploading(&self, task_id: &str, worker_id: &str) -> Result<()> {
        self.run_task_status_update(TaskStatusUpdate::Uploading, task_id, worker_id)
            .await
    }

    pub async fn mark_task_completed(&self, task_id: &str, worker_id: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite mark_task_completed connection")?;
                db::mark_task_completed(&conn, task_id, worker_id).context("sqlite mark_task_completed")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE task_queue
                     SET status = 'completed',
                         worker_id = NULL,
                         leased_at = NULL,
                         lease_expires_at = NULL,
                         last_heartbeat_at = NULL,
                         finished_at = NOW(),
                         updated_at = NOW()
                     WHERE id = $1
                       AND worker_id = $2
                       AND status IN ('leased', 'processing', 'uploading')",
                )
                .bind(task_id)
                .bind(worker_id)
                .execute(pg_pool)
                .await
                .context("postgres mark_task_completed")?;
                Ok(())
            }
        }
    }

    pub async fn mark_task_failed(
        &self,
        task_id: &str,
        worker_id: &str,
        error_message: &str,
        retryable: bool,
        max_retries: i32,
    ) -> Result<bool> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite mark_task_failed connection")?;
                db::mark_task_failed(&conn, task_id, worker_id, error_message, retryable, max_retries)
                    .context("sqlite mark_task_failed")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query("SELECT retry_count + 1 AS next_retry_count FROM task_queue WHERE id = $1")
                    .bind(task_id)
                    .fetch_one(pg_pool)
                    .await
                    .context("postgres mark_task_failed select next_retry_count")?;
                let next_retry_count: i32 = row.get("next_retry_count");
                if !retryable || next_retry_count >= max_retries {
                    sqlx::query(
                        "UPDATE task_queue
                         SET status = 'dead_letter',
                             error_message = $1,
                             retry_count = retry_count + 1,
                             worker_id = NULL,
                             leased_at = NULL,
                             lease_expires_at = NULL,
                             last_heartbeat_at = NULL,
                             finished_at = NOW(),
                             updated_at = NOW()
                         WHERE id = $2
                           AND worker_id = $3",
                    )
                    .bind(error_message)
                    .bind(task_id)
                    .bind(worker_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres mark_task_failed dead_letter")?;
                    return Ok(false);
                }

                let delay_seconds = retry_delay_seconds(next_retry_count);
                sqlx::query(
                    "UPDATE task_queue
                     SET status = 'pending',
                         error_message = $1,
                         retry_count = retry_count + 1,
                         worker_id = NULL,
                         leased_at = NULL,
                         lease_expires_at = NULL,
                         last_heartbeat_at = NULL,
                         execute_at = NOW() + ($2::bigint * INTERVAL '1 second'),
                         updated_at = NOW()
                     WHERE id = $3
                       AND worker_id = $4",
                )
                .bind(error_message)
                .bind(delay_seconds)
                .bind(task_id)
                .bind(worker_id)
                .execute(pg_pool)
                .await
                .context("postgres mark_task_failed retryable")?;
                Ok(true)
            }
        }
    }

    /// Resets all in-progress tasks back to `pending` unconditionally.
    /// Called once at startup when no worker from the previous session is still alive.
    /// Returns the number of tasks reset.
    pub async fn reset_in_progress_tasks_at_startup(&self) -> Result<u64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn =
                    db::get_connection(db_pool).context("sqlite reset_in_progress_tasks_at_startup connection")?;
                Ok(
                    db::reset_in_progress_tasks_at_startup(&conn)
                        .context("sqlite reset_in_progress_tasks_at_startup")? as u64,
                )
            }
            Self::Postgres { pg_pool, .. } => Ok(sqlx::query(
                "UPDATE task_queue
                 SET status = 'pending',
                     worker_id = NULL,
                     leased_at = NULL,
                     lease_expires_at = NULL,
                     last_heartbeat_at = NULL,
                     execute_at = NULL,
                     updated_at = NOW()
                 WHERE status IN ('leased', 'processing', 'uploading')
                   AND created_at > NOW() - INTERVAL '1 day'",
            )
            .execute(pg_pool)
            .await
            .context("postgres reset_in_progress_tasks_at_startup")?
            .rows_affected()),
        }
    }

    pub async fn recover_expired_leases(&self, max_retries: i32) -> Result<u64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite recover_expired_leases connection")?;
                Ok(db::recover_expired_leases(&conn, max_retries).context("sqlite recover_expired_leases")? as u64)
            }
            Self::Postgres { pg_pool, .. } => Ok(sqlx::query(
                "UPDATE task_queue
                 SET status = CASE
                        WHEN retry_count + 1 >= $1 THEN 'dead_letter'
                        ELSE 'pending'
                     END,
                     retry_count = retry_count + 1,
                     error_message = COALESCE(error_message, 'Lease expired'),
                     worker_id = NULL,
                     leased_at = NULL,
                     lease_expires_at = NULL,
                     last_heartbeat_at = NULL,
                     execute_at = CASE
                        WHEN retry_count + 1 >= $1 THEN execute_at
                        ELSE NOW() + INTERVAL '30 seconds'
                     END,
                     finished_at = CASE
                        WHEN retry_count + 1 >= $1 THEN NOW()
                        ELSE finished_at
                     END,
                     updated_at = NOW()
                 WHERE status IN ('leased', 'processing', 'uploading')
                   AND lease_expires_at IS NOT NULL
                   AND lease_expires_at <= NOW()",
            )
            .bind(max_retries)
            .execute(pg_pool)
            .await
            .context("postgres recover_expired_leases")?
            .rows_affected()),
        }
    }

    pub async fn count_active_tasks(&self) -> Result<usize> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite count_active_tasks connection")?;
                db::count_active_tasks(&conn).context("sqlite count_active_tasks")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT COUNT(*)::bigint AS count
                     FROM task_queue
                     WHERE status IN ('pending', 'leased', 'processing', 'uploading')",
                )
                .fetch_one(pg_pool)
                .await
                .context("postgres count_active_tasks")?;
                Ok(row.get::<i64, _>("count") as usize)
            }
        }
    }

    pub async fn get_queue_position(&self, user_id: i64) -> Result<Option<usize>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_queue_position connection")?;
                db::get_queue_position(&conn, user_id).context("sqlite get_queue_position")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "WITH target AS (
                        SELECT priority, created_at
                        FROM task_queue
                        WHERE user_id = $1 AND status = 'pending'
                        ORDER BY priority DESC, created_at ASC
                        LIMIT 1
                    )
                    SELECT COUNT(*)::bigint + 1 AS position
                    FROM task_queue, target
                    WHERE status = 'pending'
                      AND (
                        task_queue.priority > target.priority OR
                        (task_queue.priority = target.priority AND task_queue.created_at < target.created_at)
                      )",
                )
                .bind(user_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_queue_position")?;
                Ok(row.map(|row| row.get::<i64, _>("position") as usize))
            }
        }
    }

    pub async fn get_pending_tasks_for_user(&self, user_id: i64) -> Result<Vec<TaskQueueEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_pending_tasks_for_user connection")?;
                db::get_pending_tasks_for_user(&conn, user_id).context("sqlite get_pending_tasks_for_user")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT *
                     FROM task_queue
                     WHERE user_id = $1
                       AND status = 'pending'
                     ORDER BY priority DESC, created_at ASC",
                )
                .bind(user_id)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_pending_tasks_for_user")?;
                rows.into_iter().map(map_pg_task_queue_entry).collect()
            }
        }
    }

    async fn run_task_status_update(&self, status: TaskStatusUpdate, task_id: &str, worker_id: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite run_task_status_update connection")?;
                match status {
                    TaskStatusUpdate::Processing => {
                        db::mark_task_processing(&conn, task_id, worker_id).context("sqlite mark_task_processing")
                    }
                    TaskStatusUpdate::Uploading => {
                        db::mark_task_uploading(&conn, task_id, worker_id).context("sqlite mark_task_uploading")
                    }
                }
            }
            Self::Postgres { pg_pool, .. } => {
                let query = match status {
                    TaskStatusUpdate::Processing => {
                        "UPDATE task_queue
                         SET status = 'processing',
                             started_at = COALESCE(started_at, NOW()),
                             updated_at = NOW()
                         WHERE id = $1
                           AND worker_id = $2
                           AND status IN ('leased', 'processing', 'uploading')"
                    }
                    TaskStatusUpdate::Uploading => {
                        "UPDATE task_queue
                         SET status = 'uploading',
                             updated_at = NOW()
                         WHERE id = $1
                           AND worker_id = $2
                           AND status IN ('leased', 'processing', 'uploading')"
                    }
                };
                sqlx::query(query)
                    .bind(task_id)
                    .bind(worker_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres run_task_status_update")?;
                Ok(())
            }
        }
    }
}

fn retry_delay_seconds(retry_count: i32) -> i64 {
    let capped = retry_count.clamp(1, 6) as u32;
    30 * 2_i64.pow(capped - 1)
}

fn map_pg_task_queue_entry(row: sqlx::postgres::PgRow) -> Result<TaskQueueEntry> {
    Ok(TaskQueueEntry {
        id: row.get("id"),
        user_id: row.get("user_id"),
        url: row.get("url"),
        message_id: row.get("message_id"),
        format: row.get("format"),
        is_video: row.get::<i32, _>("is_video") == 1,
        video_quality: row.get("video_quality"),
        audio_bitrate: row.get("audio_bitrate"),
        time_range_start: row.get("time_range_start"),
        time_range_end: row.get("time_range_end"),
        carousel_mask: row.get::<Option<i32>, _>("carousel_mask").map(|value| value as u32),
        priority: row.get("priority"),
        status: row.get("status"),
        error_message: row.get("error_message"),
        retry_count: row.get("retry_count"),
        idempotency_key: row.get("idempotency_key"),
        worker_id: row.get("worker_id"),
        leased_at: row.try_get("leased_at").ok(),
        lease_expires_at: row.try_get("lease_expires_at").ok(),
        last_heartbeat_at: row.try_get("last_heartbeat_at").ok(),
        execute_at: row.try_get("execute_at").ok(),
        started_at: row.try_get("started_at").ok(),
        finished_at: row.try_get("finished_at").ok(),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}
