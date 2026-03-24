//! Download task queue operations.

use super::DbConnection;
use rusqlite::{OptionalExtension, Result};

/// Structure for a task entry in the DB queue
#[derive(Debug, Clone)]
pub struct TaskQueueEntry {
    pub id: String,
    pub user_id: i64,
    pub url: String,
    pub message_id: Option<i32>,
    pub format: String,
    pub is_video: bool,
    pub video_quality: Option<String>,
    pub audio_bitrate: Option<String>,
    pub time_range_start: Option<String>,
    pub time_range_end: Option<String>,
    pub carousel_mask: Option<u32>,
    pub priority: i32,
    pub status: String,
    pub error_message: Option<String>,
    pub retry_count: i32,
    pub idempotency_key: Option<String>,
    pub worker_id: Option<String>,
    pub leased_at: Option<String>,
    pub lease_expires_at: Option<String>,
    pub last_heartbeat_at: Option<String>,
    pub execute_at: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Result of attempting to enqueue a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueResult {
    Enqueued,
    Duplicate,
}

fn map_task_queue_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskQueueEntry> {
    Ok(TaskQueueEntry {
        id: row.get(0)?,
        user_id: row.get(1)?,
        url: row.get(2)?,
        message_id: row.get(3)?,
        format: row.get(4)?,
        is_video: row.get::<_, i32>(5)? == 1,
        video_quality: row.get(6)?,
        audio_bitrate: row.get(7)?,
        time_range_start: row.get(8)?,
        time_range_end: row.get(9)?,
        carousel_mask: row.get(10)?,
        priority: row.get(11)?,
        status: row.get(12)?,
        error_message: row.get(13)?,
        retry_count: row.get(14)?,
        idempotency_key: row.get(15)?,
        worker_id: row.get(16)?,
        leased_at: row.get(17)?,
        lease_expires_at: row.get(18)?,
        last_heartbeat_at: row.get(19)?,
        execute_at: row.get(20)?,
        started_at: row.get(21)?,
        finished_at: row.get(22)?,
        created_at: row.get(23)?,
        updated_at: row.get(24)?,
    })
}

fn task_queue_select_sql() -> &'static str {
    "SELECT id, user_id, url, message_id, format, is_video, video_quality, audio_bitrate,
            time_range_start, time_range_end, carousel_mask, priority, status, error_message,
            retry_count, idempotency_key, worker_id, leased_at, lease_expires_at,
            last_heartbeat_at, execute_at, started_at, finished_at, created_at, updated_at
     FROM task_queue"
}

/// Saves a task to the DB queue
#[allow(clippy::too_many_arguments)]
pub fn save_task_to_queue(
    conn: &DbConnection,
    task_id: &str,
    user_id: i64,
    url: &str,
    message_id: Option<i32>,
    format: &str,
    is_video: bool,
    video_quality: Option<&str>,
    audio_bitrate: Option<&str>,
    time_range_start: Option<&str>,
    time_range_end: Option<&str>,
    carousel_mask: Option<u32>,
    priority: i32,
    idempotency_key: &str,
) -> Result<EnqueueResult> {
    let result = conn.execute(
        "INSERT INTO task_queue (
             id, user_id, url, message_id, format, is_video, video_quality, audio_bitrate,
             time_range_start, time_range_end, carousel_mask, priority, status, retry_count, idempotency_key
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'pending', 0, ?13)",
        [
            &task_id as &dyn rusqlite::ToSql,
            &user_id as &dyn rusqlite::ToSql,
            &url as &dyn rusqlite::ToSql,
            &message_id as &dyn rusqlite::ToSql,
            &format as &dyn rusqlite::ToSql,
            &(if is_video { 1 } else { 0 }) as &dyn rusqlite::ToSql,
            &video_quality as &dyn rusqlite::ToSql,
            &audio_bitrate as &dyn rusqlite::ToSql,
            &time_range_start as &dyn rusqlite::ToSql,
            &time_range_end as &dyn rusqlite::ToSql,
            &carousel_mask as &dyn rusqlite::ToSql,
            &priority as &dyn rusqlite::ToSql,
            &idempotency_key as &dyn rusqlite::ToSql,
        ],
    );
    match result {
        Ok(_) => Ok(EnqueueResult::Enqueued),
        Err(rusqlite::Error::SqliteFailure(err, _)) if err.code == rusqlite::ffi::ErrorCode::ConstraintViolation => {
            Ok(EnqueueResult::Duplicate)
        }
        Err(e) => Err(e),
    }
}

/// Updates the status of a task
pub fn update_task_status(conn: &DbConnection, task_id: &str, status: &str, error_message: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE task_queue
         SET status = ?1,
             error_message = ?2,
             updated_at = CURRENT_TIMESTAMP
         WHERE id = ?3",
        [
            &status as &dyn rusqlite::ToSql,
            &error_message as &dyn rusqlite::ToSql,
            &task_id as &dyn rusqlite::ToSql,
        ],
    )?;
    Ok(())
}

pub fn mark_task_uploading(conn: &DbConnection, task_id: &str, worker_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE task_queue
         SET status = 'uploading',
             updated_at = CURRENT_TIMESTAMP
         WHERE id = ?1
           AND worker_id = ?2
           AND status IN ('leased', 'processing', 'uploading')",
        [&task_id as &dyn rusqlite::ToSql, &worker_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

fn retry_delay_seconds(retry_count: i32) -> i64 {
    let capped = retry_count.clamp(1, 6) as u32;
    30 * 2_i64.pow(capped - 1)
}

/// Gets a task by ID
pub fn get_task_by_id(conn: &DbConnection, task_id: &str) -> Result<Option<TaskQueueEntry>> {
    let sql = format!("{} WHERE id = ?1", task_queue_select_sql());
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map([&task_id as &dyn rusqlite::ToSql], map_task_queue_entry)?;

    if let Some(row) = rows.next() {
        Ok(Some(row?))
    } else {
        Ok(None)
    }
}

/// Claims the next runnable task using an SQLite immediate transaction.
pub fn claim_next_task(conn: &DbConnection, worker_id: &str, lease_seconds: i64) -> Result<Option<TaskQueueEntry>> {
    conn.execute_batch("BEGIN IMMEDIATE TRANSACTION")?;

    let result = (|| -> Result<Option<TaskQueueEntry>> {
        let sql = format!(
            "{} WHERE id = (
                SELECT id
                FROM task_queue
                WHERE status = 'pending'
                  AND (execute_at IS NULL OR execute_at <= CURRENT_TIMESTAMP)
                ORDER BY priority DESC, created_at ASC
                LIMIT 1
            )",
            task_queue_select_sql()
        );
        let mut stmt = conn.prepare(&sql)?;
        let next_task = stmt.query_row([], map_task_queue_entry).optional()?;

        let Some(task) = next_task else {
            return Ok(None);
        };

        conn.execute(
            "UPDATE task_queue
             SET status = 'leased',
                 worker_id = ?1,
                 leased_at = CURRENT_TIMESTAMP,
                 lease_expires_at = datetime('now', ?2),
                 last_heartbeat_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?3",
            rusqlite::params![worker_id, format!("+{} seconds", lease_seconds), task.id],
        )?;

        get_task_by_id(conn, &task.id)
    })();

    match result {
        Ok(task) => {
            conn.execute_batch("COMMIT")?;
            Ok(task)
        }
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(err)
        }
    }
}

/// Marks a task as processing
pub fn mark_task_processing(conn: &DbConnection, task_id: &str, worker_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE task_queue
         SET status = 'processing',
             started_at = COALESCE(started_at, CURRENT_TIMESTAMP),
             updated_at = CURRENT_TIMESTAMP
         WHERE id = ?1
           AND worker_id = ?2
           AND status IN ('leased', 'processing', 'uploading')",
        [&task_id as &dyn rusqlite::ToSql, &worker_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

pub fn heartbeat_worker_leases(conn: &DbConnection, worker_id: &str, lease_seconds: i64) -> Result<usize> {
    conn.execute(
        "UPDATE task_queue
         SET lease_expires_at = datetime('now', ?1),
             last_heartbeat_at = CURRENT_TIMESTAMP,
             updated_at = CURRENT_TIMESTAMP
         WHERE worker_id = ?2
           AND status IN ('leased', 'processing', 'uploading')",
        rusqlite::params![format!("+{} seconds", lease_seconds), worker_id],
    )
}

pub fn release_task(conn: &DbConnection, task_id: &str, worker_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE task_queue
         SET status = 'pending',
             error_message = NULL,
             worker_id = NULL,
             leased_at = NULL,
             lease_expires_at = NULL,
             last_heartbeat_at = NULL,
             execute_at = NULL,
             updated_at = CURRENT_TIMESTAMP
         WHERE id = ?1
           AND worker_id = ?2
           AND status IN ('leased', 'processing', 'uploading')",
        [&task_id as &dyn rusqlite::ToSql, &worker_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

pub fn mark_task_completed(conn: &DbConnection, task_id: &str, worker_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE task_queue
         SET status = 'completed',
             worker_id = NULL,
             leased_at = NULL,
             lease_expires_at = NULL,
             last_heartbeat_at = NULL,
             finished_at = CURRENT_TIMESTAMP,
             updated_at = CURRENT_TIMESTAMP
         WHERE id = ?1
           AND worker_id = ?2
           AND status IN ('leased', 'processing', 'uploading')",
        [&task_id as &dyn rusqlite::ToSql, &worker_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

pub fn mark_task_failed(
    conn: &DbConnection,
    task_id: &str,
    worker_id: &str,
    error_message: &str,
    retryable: bool,
    max_retries: i32,
) -> Result<bool> {
    let next_retry_count: i32 = conn.query_row(
        "SELECT retry_count + 1 FROM task_queue WHERE id = ?1",
        [&task_id as &dyn rusqlite::ToSql],
        |row| row.get(0),
    )?;

    if !retryable || next_retry_count >= max_retries {
        conn.execute(
            "UPDATE task_queue
             SET status = 'dead_letter',
                 error_message = ?1,
                 retry_count = retry_count + 1,
                 worker_id = NULL,
                 leased_at = NULL,
                 lease_expires_at = NULL,
                 last_heartbeat_at = NULL,
                 finished_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?2
               AND worker_id = ?3",
            [
                &error_message as &dyn rusqlite::ToSql,
                &task_id as &dyn rusqlite::ToSql,
                &worker_id as &dyn rusqlite::ToSql,
            ],
        )?;
        return Ok(false);
    }

    let delay_seconds = retry_delay_seconds(next_retry_count);
    conn.execute(
        "UPDATE task_queue
         SET status = 'pending',
             error_message = ?1,
             retry_count = retry_count + 1,
             worker_id = NULL,
             leased_at = NULL,
             lease_expires_at = NULL,
             last_heartbeat_at = NULL,
             execute_at = datetime('now', ?2),
             updated_at = CURRENT_TIMESTAMP
         WHERE id = ?3
           AND worker_id = ?4",
        rusqlite::params![error_message, format!("+{} seconds", delay_seconds), task_id, worker_id],
    )?;
    Ok(true)
}

/// Resets all in-progress tasks (`leased`, `processing`, `uploading`) back to `pending`
/// unconditionally, regardless of lease expiry. Called once at startup when we know
/// no worker from the previous session is still alive.
/// Returns the number of tasks reset.
pub fn reset_in_progress_tasks_at_startup(conn: &DbConnection) -> Result<usize> {
    conn.execute(
        "UPDATE task_queue
         SET status = 'pending',
             worker_id = NULL,
             leased_at = NULL,
             lease_expires_at = NULL,
             last_heartbeat_at = NULL,
             execute_at = NULL,
             updated_at = CURRENT_TIMESTAMP
         WHERE status IN ('leased', 'processing', 'uploading')
           AND created_at > datetime('now', '-1 day')",
        [],
    )
}

pub fn recover_expired_leases(conn: &DbConnection, max_retries: i32) -> Result<usize> {
    conn.execute(
        "UPDATE task_queue
         SET status = CASE
                WHEN retry_count + 1 >= ?1 THEN 'dead_letter'
                ELSE 'pending'
             END,
             retry_count = retry_count + 1,
             error_message = COALESCE(error_message, 'Lease expired'),
             worker_id = NULL,
             leased_at = NULL,
             lease_expires_at = NULL,
             last_heartbeat_at = NULL,
             execute_at = CASE
                WHEN retry_count + 1 >= ?1 THEN execute_at
                ELSE datetime('now', '+30 seconds')
             END,
             finished_at = CASE
                WHEN retry_count + 1 >= ?1 THEN CURRENT_TIMESTAMP
                ELSE finished_at
             END,
             updated_at = CURRENT_TIMESTAMP
         WHERE status IN ('leased', 'processing', 'uploading')
           AND lease_expires_at IS NOT NULL
           AND lease_expires_at <= CURRENT_TIMESTAMP",
        [&max_retries as &dyn rusqlite::ToSql],
    )
}

pub fn count_active_tasks(conn: &DbConnection) -> Result<usize> {
    conn.query_row(
        "SELECT COUNT(*) FROM task_queue WHERE status IN ('pending', 'leased', 'processing', 'uploading')",
        [],
        |row| row.get(0),
    )
}

pub fn get_queue_position(conn: &DbConnection, user_id: i64) -> Result<Option<usize>> {
    let task = conn
        .query_row(
            "SELECT priority, created_at
             FROM task_queue
             WHERE user_id = ?1
               AND status = 'pending'
             ORDER BY priority DESC, created_at ASC
             LIMIT 1",
            [&user_id as &dyn rusqlite::ToSql],
            |row| Ok((row.get::<_, i32>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;

    let Some((priority, created_at)) = task else {
        return Ok(None);
    };

    let ahead: usize = conn.query_row(
        "SELECT COUNT(*) FROM task_queue
         WHERE status = 'pending'
           AND (
                priority > ?1 OR
                (priority = ?1 AND created_at < ?2)
           )",
        rusqlite::params![priority, created_at],
        |row| row.get(0),
    )?;
    Ok(Some(ahead + 1))
}

pub fn get_pending_tasks_for_user(conn: &DbConnection, user_id: i64) -> Result<Vec<TaskQueueEntry>> {
    let sql = format!(
        "{} WHERE user_id = ?1
           AND status = 'pending'
           ORDER BY priority DESC, created_at ASC",
        task_queue_select_sql()
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([&user_id as &dyn rusqlite::ToSql], map_task_queue_entry)?;

    let mut tasks = Vec::new();
    for row in rows {
        tasks.push(row?);
    }
    Ok(tasks)
}

/// Gets terminally failed tasks for inspection.
pub fn get_failed_tasks(conn: &DbConnection, max_retries: i32) -> Result<Vec<TaskQueueEntry>> {
    let sql = format!(
        "{} WHERE status = 'dead_letter'
           AND retry_count >= ?1
           ORDER BY priority DESC, created_at ASC",
        task_queue_select_sql()
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([&max_retries as &dyn rusqlite::ToSql], map_task_queue_entry)?;

    let mut tasks = Vec::new();
    for row in rows {
        tasks.push(row?);
    }
    Ok(tasks)
}

/// Legacy helper kept for compatibility with older startup/tests.
pub fn get_and_reset_recoverable_tasks(conn: &DbConnection) -> Result<Vec<TaskQueueEntry>> {
    recover_expired_leases(conn, i32::MAX)?;
    let sql = format!(
        "{} WHERE status = 'pending'
           AND created_at > datetime('now', '-1 day')
           ORDER BY priority DESC, created_at ASC",
        task_queue_select_sql()
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], map_task_queue_entry)?;

    let mut tasks = Vec::new();
    for row in rows {
        tasks.push(row?);
    }
    Ok(tasks)
}

pub fn register_processed_update(conn: &DbConnection, bot_id: i64, update_id: i64) -> Result<bool> {
    let changed = conn.execute(
        "INSERT INTO processed_updates (bot_id, update_id) VALUES (?1, ?2)
         ON CONFLICT(bot_id, update_id) DO NOTHING",
        rusqlite::params![bot_id, update_id],
    )?;
    Ok(changed == 1)
}

/// Removes completed and failed tasks older than `days` from task_queue.
/// Returns the number of rows deleted.
pub fn cleanup_old_tasks(conn: &DbConnection, days: i64) -> Result<usize> {
    let deleted = conn.execute(
        "DELETE FROM task_queue
         WHERE status IN ('completed', 'dead_letter')
         AND updated_at < datetime('now', ?1)",
        [&format!("-{} days", days)],
    )?;
    Ok(deleted)
}

pub fn cleanup_old_processed_updates(conn: &DbConnection, hours: i64) -> Result<usize> {
    conn.execute(
        "DELETE FROM processed_updates
         WHERE created_at < datetime('now', ?1)",
        [&format!("-{} hours", hours)],
    )
}
