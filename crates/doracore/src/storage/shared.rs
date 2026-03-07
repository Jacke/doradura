use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};

use crate::core::config::{self, DatabaseDriver};
use crate::core::types::Plan;
use crate::storage::db;
use crate::storage::db::{DbPool, EnqueueResult, SubtitleStyle, TaskQueueEntry, User};

const POSTGRES_BOOTSTRAP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS users (
    telegram_id BIGINT PRIMARY KEY,
    username TEXT,
    plan TEXT NOT NULL DEFAULT 'free',
    download_format TEXT NOT NULL DEFAULT 'mp3',
    download_subtitles INTEGER NOT NULL DEFAULT 0,
    video_quality TEXT NOT NULL DEFAULT 'best',
    audio_bitrate TEXT NOT NULL DEFAULT '320k',
    language TEXT NOT NULL DEFAULT 'en',
    send_as_document INTEGER NOT NULL DEFAULT 0,
    send_audio_as_document INTEGER NOT NULL DEFAULT 0,
    burn_subtitles INTEGER NOT NULL DEFAULT 0,
    progress_bar_style TEXT NOT NULL DEFAULT 'classic',
    subtitle_font_size TEXT NOT NULL DEFAULT 'medium',
    subtitle_text_color TEXT NOT NULL DEFAULT 'white',
    subtitle_outline_color TEXT NOT NULL DEFAULT 'black',
    subtitle_outline_width INTEGER NOT NULL DEFAULT 2,
    subtitle_shadow INTEGER NOT NULL DEFAULT 1,
    subtitle_position TEXT NOT NULL DEFAULT 'bottom',
    is_blocked INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS subscriptions (
    user_id BIGINT PRIMARY KEY REFERENCES users(telegram_id) ON DELETE CASCADE,
    plan TEXT NOT NULL DEFAULT 'free',
    expires_at TIMESTAMPTZ,
    telegram_charge_id TEXT,
    is_recurring INTEGER NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS bot_assets (
    key TEXT PRIMARY KEY,
    file_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS task_queue (
    id TEXT PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(telegram_id) ON DELETE CASCADE,
    url TEXT NOT NULL,
    message_id INTEGER,
    format TEXT NOT NULL,
    is_video INTEGER NOT NULL DEFAULT 0,
    video_quality TEXT,
    audio_bitrate TEXT,
    time_range_start TEXT,
    time_range_end TEXT,
    carousel_mask INTEGER,
    priority INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'pending',
    error_message TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0,
    idempotency_key TEXT,
    worker_id TEXT,
    leased_at TIMESTAMPTZ,
    lease_expires_at TIMESTAMPTZ,
    last_heartbeat_at TIMESTAMPTZ,
    execute_at TIMESTAMPTZ,
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_task_queue_runnable
    ON task_queue(status, priority DESC, created_at ASC);
CREATE INDEX IF NOT EXISTS idx_task_queue_lease_expiry
    ON task_queue(status, lease_expires_at);
CREATE INDEX IF NOT EXISTS idx_task_queue_user_pending
    ON task_queue(user_id, status, created_at ASC);
CREATE UNIQUE INDEX IF NOT EXISTS idx_task_queue_active_idempotency
    ON task_queue(idempotency_key)
    WHERE idempotency_key IS NOT NULL
      AND status IN ('pending', 'leased', 'processing', 'uploading');

CREATE TABLE IF NOT EXISTS processed_updates (
    bot_id BIGINT NOT NULL,
    update_id BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (bot_id, update_id)
);

CREATE INDEX IF NOT EXISTS idx_processed_updates_created_at
    ON processed_updates(created_at);
"#;

#[derive(Debug, Clone)]
pub struct QueueTaskInput<'a> {
    pub task_id: &'a str,
    pub user_id: i64,
    pub url: &'a str,
    pub message_id: Option<i32>,
    pub format: &'a str,
    pub is_video: bool,
    pub video_quality: Option<&'a str>,
    pub audio_bitrate: Option<&'a str>,
    pub time_range_start: Option<&'a str>,
    pub time_range_end: Option<&'a str>,
    pub carousel_mask: Option<u32>,
    pub priority: i32,
    pub idempotency_key: &'a str,
}

#[derive(Clone)]
pub enum SharedStorage {
    Sqlite { db_pool: Arc<DbPool> },
    Postgres { sqlite_pool: Arc<DbPool>, pg_pool: PgPool },
}

impl SharedStorage {
    pub async fn from_sqlite_pool(db_pool: Arc<DbPool>) -> Result<Arc<Self>> {
        match *config::DATABASE_DRIVER {
            DatabaseDriver::Sqlite => Ok(Arc::new(Self::Sqlite { db_pool })),
            DatabaseDriver::Postgres => {
                let database_url = config::DATABASE_URL
                    .clone()
                    .ok_or_else(|| anyhow!("DATABASE_URL must be set when DATABASE_DRIVER=postgres"))?;
                let pg_pool = PgPoolOptions::new()
                    .max_connections(20)
                    .acquire_timeout(Duration::from_secs(3))
                    .connect(&database_url)
                    .await
                    .context("connect postgres shared storage")?;
                sqlx::query(POSTGRES_BOOTSTRAP_SQL)
                    .execute(&pg_pool)
                    .await
                    .context("bootstrap postgres shared storage schema")?;
                Ok(Arc::new(Self::Postgres {
                    sqlite_pool: db_pool,
                    pg_pool,
                }))
            }
        }
    }

    pub fn sqlite_pool(&self) -> Arc<DbPool> {
        match self {
            Self::Sqlite { db_pool } => Arc::clone(db_pool),
            Self::Postgres { sqlite_pool, .. } => Arc::clone(sqlite_pool),
        }
    }

    pub fn is_postgres(&self) -> bool {
        matches!(self, Self::Postgres { .. })
    }

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
        self.run_task_status_update(
            "UPDATE task_queue
             SET status = 'processing',
                 started_at = COALESCE(started_at, NOW()),
                 updated_at = NOW()
             WHERE id = $1
               AND worker_id = $2
               AND status IN ('leased', 'processing', 'uploading')",
            task_id,
            worker_id,
        )
        .await
    }

    pub async fn mark_task_uploading(&self, task_id: &str, worker_id: &str) -> Result<()> {
        self.run_task_status_update(
            "UPDATE task_queue
             SET status = 'uploading',
                 updated_at = NOW()
             WHERE id = $1
               AND worker_id = $2
               AND status IN ('leased', 'processing', 'uploading')",
            task_id,
            worker_id,
        )
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

    pub async fn get_user(&self, telegram_id: i64) -> Result<Option<User>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user connection")?;
                db::get_user(&conn, telegram_id).context("sqlite get_user")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT
                        u.telegram_id,
                        u.username,
                        COALESCE(s.plan, u.plan) AS plan,
                        u.download_format,
                        u.download_subtitles,
                        u.video_quality,
                        u.audio_bitrate,
                        u.language,
                        u.send_as_document,
                        u.send_audio_as_document,
                        CAST(s.expires_at AS TEXT) AS subscription_expires_at,
                        s.telegram_charge_id,
                        COALESCE(s.is_recurring, 0) AS is_recurring,
                        COALESCE(u.burn_subtitles, 0) AS burn_subtitles,
                        COALESCE(u.progress_bar_style, 'classic') AS progress_bar_style,
                        COALESCE(u.is_blocked, 0) AS is_blocked
                     FROM users u
                     LEFT JOIN subscriptions s ON s.user_id = u.telegram_id
                     WHERE u.telegram_id = $1",
                )
                .bind(telegram_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_user")?;
                row.map(map_pg_user).transpose()
            }
        }
    }

    pub async fn create_user(&self, telegram_id: i64, username: Option<String>) -> Result<()> {
        self.create_user_with_language(telegram_id, username, None).await
    }

    pub async fn create_user_with_language(
        &self,
        telegram_id: i64,
        username: Option<String>,
        language: Option<&str>,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite create_user connection")?;
                if let Some(language) = language {
                    db::create_user_with_language(&conn, telegram_id, username, language)
                        .context("sqlite create_user_with_language")
                } else {
                    db::create_user(&conn, telegram_id, username).context("sqlite create_user")
                }
            }
            Self::Postgres { pg_pool, .. } => {
                let mut tx = pg_pool.begin().await.context("postgres create_user begin")?;
                sqlx::query(
                    "INSERT INTO users (
                        telegram_id, username, download_format, download_subtitles, video_quality,
                        audio_bitrate, language, send_as_document, send_audio_as_document
                     ) VALUES ($1, $2, 'mp3', 0, 'best', '320k', $3, 0, 0)
                     ON CONFLICT (telegram_id) DO NOTHING",
                )
                .bind(telegram_id)
                .bind(username)
                .bind(language.unwrap_or("en"))
                .execute(&mut *tx)
                .await
                .context("postgres create_user users insert")?;
                sqlx::query(
                    "INSERT INTO subscriptions (user_id, plan)
                     VALUES ($1, 'free')
                     ON CONFLICT (user_id) DO NOTHING",
                )
                .bind(telegram_id)
                .execute(&mut *tx)
                .await
                .context("postgres create_user subscriptions insert")?;
                tx.commit().await.context("postgres create_user commit")?;
                Ok(())
            }
        }
    }

    pub async fn get_user_language(&self, telegram_id: i64) -> Result<String> {
        self.get_user_string_setting(
            telegram_id,
            "language",
            "SELECT language FROM users WHERE telegram_id = $1",
            "ru",
        )
        .await
    }

    pub async fn get_user_progress_bar_style(&self, telegram_id: i64) -> Result<String> {
        self.get_user_string_setting(
            telegram_id,
            "progress_bar_style",
            "SELECT progress_bar_style FROM users WHERE telegram_id = $1",
            "classic",
        )
        .await
    }

    pub async fn get_user_video_quality(&self, telegram_id: i64) -> Result<String> {
        self.get_user_string_setting(
            telegram_id,
            "video_quality",
            "SELECT video_quality FROM users WHERE telegram_id = $1",
            "best",
        )
        .await
    }

    pub async fn get_user_audio_bitrate(&self, telegram_id: i64) -> Result<String> {
        self.get_user_string_setting(
            telegram_id,
            "audio_bitrate",
            "SELECT audio_bitrate FROM users WHERE telegram_id = $1",
            "320k",
        )
        .await
    }

    pub async fn get_user_send_as_document(&self, telegram_id: i64) -> Result<i32> {
        self.get_user_i32_setting(
            telegram_id,
            "send_as_document",
            "SELECT send_as_document FROM users WHERE telegram_id = $1",
            0,
        )
        .await
    }

    pub async fn get_user_send_audio_as_document(&self, telegram_id: i64) -> Result<i32> {
        self.get_user_i32_setting(
            telegram_id,
            "send_audio_as_document",
            "SELECT send_audio_as_document FROM users WHERE telegram_id = $1",
            0,
        )
        .await
    }

    pub async fn get_user_download_subtitles(&self, telegram_id: i64) -> Result<bool> {
        Ok(self
            .get_user_i32_setting(
                telegram_id,
                "download_subtitles",
                "SELECT download_subtitles FROM users WHERE telegram_id = $1",
                0,
            )
            .await?
            == 1)
    }

    pub async fn get_user_burn_subtitles(&self, telegram_id: i64) -> Result<bool> {
        Ok(self
            .get_user_i32_setting(
                telegram_id,
                "burn_subtitles",
                "SELECT COALESCE(burn_subtitles, 0) FROM users WHERE telegram_id = $1",
                0,
            )
            .await?
            == 1)
    }

    pub async fn get_user_subtitle_style(&self, telegram_id: i64) -> Result<SubtitleStyle> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user_subtitle_style connection")?;
                db::get_user_subtitle_style(&conn, telegram_id).context("sqlite get_user_subtitle_style")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT
                        COALESCE(subtitle_font_size, 'medium') AS subtitle_font_size,
                        COALESCE(subtitle_text_color, 'white') AS subtitle_text_color,
                        COALESCE(subtitle_outline_color, 'black') AS subtitle_outline_color,
                        COALESCE(subtitle_outline_width, 2) AS subtitle_outline_width,
                        COALESCE(subtitle_shadow, 1) AS subtitle_shadow,
                        COALESCE(subtitle_position, 'bottom') AS subtitle_position
                     FROM users
                     WHERE telegram_id = $1",
                )
                .bind(telegram_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_user_subtitle_style")?;
                Ok(row.map(map_pg_subtitle_style).unwrap_or_default())
            }
        }
    }

    pub async fn get_bot_asset(&self, key: &str) -> Result<Option<String>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_bot_asset connection")?;
                db::get_bot_asset(&conn, key).context("sqlite get_bot_asset")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query("SELECT file_id FROM bot_assets WHERE key = $1")
                    .bind(key)
                    .fetch_optional(pg_pool)
                    .await
                    .context("postgres get_bot_asset")?;
                Ok(row.map(|row| row.get("file_id")))
            }
        }
    }

    pub async fn set_bot_asset(&self, key: &str, file_id: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite set_bot_asset connection")?;
                db::set_bot_asset(&conn, key, file_id).context("sqlite set_bot_asset")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO bot_assets (key, file_id, created_at)
                     VALUES ($1, $2, NOW())
                     ON CONFLICT (key) DO UPDATE SET file_id = EXCLUDED.file_id, created_at = NOW()",
                )
                .bind(key)
                .bind(file_id)
                .execute(pg_pool)
                .await
                .context("postgres set_bot_asset")?;
                Ok(())
            }
        }
    }

    async fn get_user_string_setting(
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

    async fn get_user_i32_setting(
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

    async fn run_task_status_update(&self, postgres_query: &str, task_id: &str, worker_id: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite run_task_status_update connection")?;
                if postgres_query.contains("processing") {
                    db::mark_task_processing(&conn, task_id, worker_id).context("sqlite mark_task_processing")
                } else {
                    db::mark_task_uploading(&conn, task_id, worker_id).context("sqlite mark_task_uploading")
                }
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(postgres_query)
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

fn map_pg_user(row: sqlx::postgres::PgRow) -> Result<User> {
    let plan_raw: String = row.get("plan");
    let plan = Plan::from_str(plan_raw.as_str()).map_err(|err| anyhow!("parse user plan: {}", err))?;
    Ok(User {
        telegram_id: row.get("telegram_id"),
        username: row.get("username"),
        plan,
        download_format: row.get("download_format"),
        download_subtitles: row.get("download_subtitles"),
        video_quality: row.get("video_quality"),
        audio_bitrate: row.get("audio_bitrate"),
        language: row.get("language"),
        send_as_document: row.get("send_as_document"),
        send_audio_as_document: row.get("send_audio_as_document"),
        subscription_expires_at: row.get("subscription_expires_at"),
        telegram_charge_id: row.get("telegram_charge_id"),
        is_recurring: row.get::<i32, _>("is_recurring") != 0,
        burn_subtitles: row.get("burn_subtitles"),
        progress_bar_style: row.get("progress_bar_style"),
        is_blocked: row.get::<i32, _>("is_blocked") != 0,
    })
}

fn map_pg_subtitle_style(row: sqlx::postgres::PgRow) -> SubtitleStyle {
    SubtitleStyle {
        font_size: row.get("subtitle_font_size"),
        text_color: row.get("subtitle_text_color"),
        outline_color: row.get("subtitle_outline_color"),
        outline_width: row.get("subtitle_outline_width"),
        shadow: row.get("subtitle_shadow"),
        position: row.get("subtitle_position"),
    }
}
