//! Database access layer.
//!
//! # Module structure
//!
//! | Section | Line range | Description |
//! |---------|-----------|-------------|
//! | **Pool & Connection** | top | `DbPool`, `DbConnection`, `create_pool`, `get_connection` |
//! | **Users** | `create_user`…`set_user_language` | CRUD, settings, plan |
//! | **Subtitle Style** | `SubtitleStyle` | Burn-in subtitle preferences |
//! | **Download History** | `DownloadHistoryEntry`…`get_download_history_filtered` | History, stats, export |
//! | **Categories** | `create_user_category`…`delete_new_category_session` | User-defined categories |
//! | **Task Queue** | `TaskQueueEntry`…`mark_task_processing` | Download task queue |
//! | **Sessions** | → `sessions` submodule | Audio effects, audio cut, video clip, cookies upload |
//! | **Bot Assets** | `get_bot_asset`, `set_bot_asset` | Cached Telegram file IDs |
//! | **Video Timestamps** | `VideoTimestamp`…`delete_video_timestamps` | Chapter markers |
//! | **Cuts** | `CutEntry`…`get_cut_entry` | Audio/video clip history |
//! | **Subscriptions** | `Subscription`…`cancel_subscription` | Plan & payment management |
//! | **Charges** | `save_charge`…`get_charges_stats` | Telegram Stars payments |
//! | **Feedback** | `FeedbackMessage`…`get_feedback_stats` | User feedback system |
//! | **Error Log** | `ErrorLogEntry`…`cleanup_old_errors` | Error tracking |
//! | **Lyrics Sessions** | `create_lyrics_session`…`delete_expired_lyrics_sessions` | Lyrics search sessions |

mod playlists;
mod sessions;
mod synced_playlists;
mod vault;
pub use playlists::*;
pub use sessions::*;
pub use synced_playlists::*;
pub use vault::*;

use crate::core::types::Plan;
use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Connection, Result};
use std::time::Duration;

use crate::storage::migrations;

/// Connection timeout for pool.get() calls - prevents indefinite blocking.
/// 3s gives enough room for SQLite busy_timeout (5s PRAGMA) while still failing fast
/// if the pool is genuinely exhausted.
const CONNECTION_TIMEOUT_SECS: u64 = 3;

/// Structure representing a user in the database.
pub struct User {
    /// Telegram ID of the user
    pub telegram_id: i64,
    /// Telegram username, if available
    pub username: Option<String>,
    /// User plan
    pub plan: Plan,
    /// Preferred download format: "mp3", "mp4", "srt", "txt"
    pub download_format: String,
    /// Subtitle download flag (0 = disabled, 1 = enabled)
    pub download_subtitles: i32,
    /// Video quality: "best", "1080p", "720p", "480p", "360p"
    pub video_quality: String,
    /// Audio bitrate: "128k", "192k", "256k", "320k"
    pub audio_bitrate: String,
    /// Video send type: 0 = Media (send_video), 1 = Document (send_document)
    pub send_as_document: i32,
    /// Audio send type: 0 = Media (send_audio), 1 = Document (send_document)
    pub send_audio_as_document: i32,
    /// Subscription expiry date (from subscriptions table)
    pub subscription_expires_at: Option<String>,
    /// Telegram payment charge ID (from subscriptions table)
    pub telegram_charge_id: Option<String>,
    /// Preferred user language (IETF tag, e.g. "ru", "en-US")
    pub language: String,
    /// Recurring subscription flag (auto-renewal) from subscriptions table
    pub is_recurring: bool,
    /// Subtitle burn-in flag for video (0 = disabled, 1 = enabled)
    pub burn_subtitles: i32,
    /// Progress bar style: "classic", "gradient", "emoji", "dots", "runner", "rpg", "fire", "moon"
    pub progress_bar_style: String,
    /// Whether the user is blocked by admin
    pub is_blocked: bool,
}

/// Structure containing user subscription data.
#[derive(Debug, Clone)]
pub struct Subscription {
    pub user_id: i64,
    pub plan: Plan,
    pub expires_at: Option<String>,
    pub telegram_charge_id: Option<String>,
    pub is_recurring: bool,
}

/// Structure containing payment (charge) data from Telegram Stars.
/// Stores complete payment information for accounting purposes.
#[derive(Debug, Clone)]
pub struct Charge {
    pub id: i64,
    pub user_id: i64,
    pub plan: Plan,
    pub telegram_charge_id: String,
    pub provider_charge_id: Option<String>,
    pub currency: String,
    pub total_amount: i64,
    pub invoice_payload: String,
    pub is_recurring: bool,
    pub is_first_recurring: bool,
    pub subscription_expiration_date: Option<String>,
    pub payment_date: String,
    pub created_at: String,
}

/// Structure containing user feedback data.
#[derive(Debug, Clone)]
pub struct FeedbackMessage {
    pub id: i64,
    pub user_id: i64,
    pub username: Option<String>,
    pub first_name: String,
    pub message: String,
    pub status: String,
    pub admin_reply: Option<String>,
    pub created_at: String,
    pub replied_at: Option<String>,
}

impl User {
    /// Returns the Telegram ID of the user.
    ///
    /// # Returns
    ///
    /// Telegram ID of the user.
    pub fn telegram_id(&self) -> i64 {
        self.telegram_id
    }

    /// Returns the preferred download format of the user.
    ///
    /// # Returns
    ///
    /// Download format: "mp3", "mp4", "srt", "txt"
    pub fn download_format(&self) -> &str {
        &self.download_format
    }
}

pub type DbPool = Pool<SqliteConnectionManager>;
pub type DbConnection = PooledConnection<SqliteConnectionManager>;

/// Create a new database connection pool
///
/// Initializes a connection pool with up to 10 connections and runs schema migrations.
///
/// # Arguments
///
/// * `database_path` - Path to SQLite database file
///
/// # Returns
///
/// Returns a `DbPool` on success or an `r2d2::Error` if pool creation fails.
///
/// # Example
///
/// ```no_run
/// use doracore::storage::db;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let pool = db::create_pool("database.sqlite")?;
/// # Ok(())
/// # }
/// ```
pub fn create_pool(database_path: &str) -> Result<DbPool, r2d2::Error> {
    let path = std::path::Path::new(database_path);
    let resolved_path = if path.is_absolute() {
        path.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(path)
    } else {
        path.to_path_buf()
    };
    log::info!("Using SQLite database at {}", resolved_path.display());

    // Run migrations before pool creation to avoid holding a pooled connection open
    match Connection::open(database_path) {
        Ok(mut conn) => {
            if let Err(e) = migrations::run_migrations(&mut conn) {
                log::warn!("Failed to run database migrations: {}", e);
            }
        }
        Err(e) => {
            log::warn!("Failed to open database for migrations: {}", e);
        }
    }

    let manager = SqliteConnectionManager::file(database_path)
        .with_init(|conn| conn.execute_batch("PRAGMA busy_timeout = 5000;"));
    let pool = Pool::builder()
        .max_size(20) // Maximum 20 connections in the pool
        .connection_timeout(Duration::from_secs(CONNECTION_TIMEOUT_SECS)) // Prevent indefinite blocking
        .build(manager)?;

    // Enable WAL mode for better concurrent read performance (~5x throughput).
    // WAL is a persistent database-level setting — only needs to be set once,
    // but re-setting is a no-op so it's safe to do on every startup.
    match pool.get() {
        Ok(conn) => {
            if let Err(e) = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;") {
                log::warn!("Failed to enable WAL mode: {}", e);
            } else {
                log::info!("SQLite WAL mode enabled");
            }
        }
        Err(e) => log::warn!("Failed to get connection for WAL setup: {}", e),
    }

    log::info!(
        "Database pool created: max_size=20, connection_timeout={}s",
        CONNECTION_TIMEOUT_SECS
    );

    Ok(pool)
}

/// Get a connection from the pool
///
/// Retrieves a connection from the connection pool. The connection is automatically
/// returned to the pool when dropped.
///
/// # Arguments
///
/// * `pool` - Database connection pool
///
/// # Returns
///
/// Returns a `DbConnection` on success or an `r2d2::Error` if no connection is available.
///
/// # Example
///
/// ```no_run
/// use doracore::storage::db;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let pool = db::create_pool("database.sqlite")?;
/// let conn = db::get_connection(&pool)?;
/// // Use connection...
/// # Ok(())
/// # }
/// ```
pub fn get_connection(pool: &DbPool) -> Result<DbConnection, r2d2::Error> {
    let state = pool.state();
    let active = state.connections - state.idle_connections;
    crate::core::metrics::DB_CONNECTIONS_ACTIVE.set(active as f64);
    crate::core::metrics::DB_CONNECTIONS_IDLE.set(state.idle_connections as f64);

    match pool.get() {
        Ok(conn) => Ok(conn),
        Err(e) => {
            log::error!(
                "DB pool exhaustion: {} (pool state: {} idle, {} in use)",
                e,
                state.idle_connections,
                active
            );
            crate::core::metrics::record_error("db_pool_timeout", "get_connection");
            Err(e)
        }
    }
}

/// Get a connection from the pool with retry and exponential backoff
///
/// Retries up to `max_retries` times with exponential backoff starting at 10ms.
/// This is useful for handling transient pool exhaustion under high load.
///
/// # Arguments
///
/// * `pool` - Database connection pool
/// * `max_retries` - Maximum number of retry attempts (recommended: 3)
///
/// # Returns
///
/// Returns a `DbConnection` on success or an `r2d2::Error` if all retries fail.
pub async fn get_connection_with_retry(pool: &DbPool, max_retries: u32) -> Result<DbConnection, r2d2::Error> {
    let mut last_error = None;
    let mut delay_ms = 10u64; // Start with 10ms

    for attempt in 0..=max_retries {
        match pool.get() {
            Ok(conn) => {
                if attempt > 0 {
                    log::debug!("DB connection acquired after {} retries", attempt);
                }
                return Ok(conn);
            }
            Err(e) => {
                last_error = Some(e);
                if attempt < max_retries {
                    log::debug!(
                        "DB pool busy, retry {}/{} in {}ms (pool: {} idle, {} in use)",
                        attempt + 1,
                        max_retries,
                        delay_ms,
                        pool.state().idle_connections,
                        pool.state().connections - pool.state().idle_connections
                    );
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    delay_ms = delay_ms.saturating_mul(2).min(500); // Cap at 500ms
                }
            }
        }
    }

    // All retries failed — last_error is always Some after the loop,
    // but we avoid expect() to be defensive.
    let e = match last_error {
        Some(e) => e,
        None => unreachable!("last_error must be set after loop"),
    };
    log::error!(
        "DB pool exhaustion after {} retries: {} (pool: {} idle, {} in use)",
        max_retries,
        e,
        pool.state().idle_connections,
        pool.state().connections - pool.state().idle_connections
    );
    crate::core::metrics::record_error("db_pool_timeout", "get_connection_with_retry");
    Err(e)
}

/// Legacy function for backward compatibility (deprecated)
/// Use get_connection(&pool) instead
#[deprecated(note = "Use get_connection(&pool) instead")]
pub fn get_connection_legacy() -> Result<Connection> {
    let mut conn = Connection::open("database.sqlite")?;
    if let Err(e) = migrations::run_migrations(&mut conn) {
        log::warn!("Failed to run migrations: {}", e);
    }
    Ok(conn)
}

/// Creates a new user in the database.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `username` - Username (optional)
///
/// # Returns
///
/// Returns `Ok(())` on success or a database error.
///
/// # Errors
///
/// Returns an error if a user with the given ID already exists or a DB error occurred.
pub fn create_user(conn: &DbConnection, telegram_id: i64, username: Option<String>) -> Result<()> {
    // Use a transaction to ensure both inserts succeed or fail together
    conn.execute_batch("BEGIN IMMEDIATE")?;

    let result = (|| {
        conn.execute(
            "INSERT INTO users (telegram_id, username, download_format, download_subtitles, video_quality, audio_bitrate, language, send_as_document, send_audio_as_document) VALUES (?1, ?2, 'mp3', 0, 'best', '320k', 'en', 0, 0)",
            [
                &telegram_id as &dyn rusqlite::ToSql,
                &username as &dyn rusqlite::ToSql,
            ],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO subscriptions (user_id, plan) VALUES (?1, 'free')",
            [&telegram_id as &dyn rusqlite::ToSql],
        )?;
        Ok(())
    })();

    match result {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(e) => {
            if let Err(rb_err) = conn.execute_batch("ROLLBACK") {
                log::error!("ROLLBACK failed: {}", rb_err);
            }
            Err(e)
        }
    }
}

/// Creates a new user in the database with the specified language.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `username` - Username (optional)
/// * `language` - Language code (e.g. "ru", "en", "fr", "de")
///
/// # Returns
///
/// Returns `Ok(())` on success or a database error.
///
/// # Errors
///
/// Returns an error if a user with the given ID already exists or a DB error occurred.
pub fn create_user_with_language(
    conn: &DbConnection,
    telegram_id: i64,
    username: Option<String>,
    language: &str,
) -> Result<()> {
    // Use a transaction to ensure both inserts succeed or fail together
    conn.execute_batch("BEGIN IMMEDIATE")?;

    let result = (|| {
        conn.execute(
            "INSERT INTO users (telegram_id, username, download_format, download_subtitles, video_quality, audio_bitrate, language, send_as_document, send_audio_as_document) VALUES (?1, ?2, 'mp3', 0, 'best', '320k', ?3, 0, 0)",
            [
                &telegram_id as &dyn rusqlite::ToSql,
                &username as &dyn rusqlite::ToSql,
                &language as &dyn rusqlite::ToSql,
            ],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO subscriptions (user_id, plan) VALUES (?1, 'free')",
            [&telegram_id as &dyn rusqlite::ToSql],
        )?;
        Ok(())
    })();

    match result {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(e) => {
            if let Err(rb_err) = conn.execute_batch("ROLLBACK") {
                log::error!("ROLLBACK failed: {}", rb_err);
            }
            Err(e)
        }
    }
}

/// Retrieves a user from the database by Telegram ID.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
///
/// # Returns
///
/// Returns `Ok(Some(User))` if the user is found, `Ok(None)` if not found,
/// or a database error.
pub fn get_user(conn: &DbConnection, telegram_id: i64) -> Result<Option<User>> {
    let mut stmt = conn.prepare(
        "SELECT
            u.telegram_id,
            u.username,
            COALESCE(s.plan, u.plan) as plan,
            u.download_format,
            u.download_subtitles,
            u.video_quality,
            u.audio_bitrate,
            u.language,
            u.send_as_document,
            u.send_audio_as_document,
            s.expires_at as subscription_expires_at,
            s.telegram_charge_id as telegram_charge_id,
            COALESCE(s.is_recurring, 0) as is_recurring,
            COALESCE(u.burn_subtitles, 0) as burn_subtitles,
            COALESCE(u.progress_bar_style, 'classic') as progress_bar_style,
            COALESCE(u.is_blocked, 0) as is_blocked
        FROM users u
        LEFT JOIN subscriptions s ON s.user_id = u.telegram_id
        WHERE u.telegram_id = ?",
    )?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        let telegram_id: i64 = row.get(0)?;
        let username: Option<String> = row.get(1)?;
        let plan: Plan = row.get(2)?;
        let download_format: String = row.get(3)?;
        let download_subtitles: i32 = row.get(4)?;
        let video_quality: String = row.get(5).unwrap_or_else(|_| "best".to_string());
        let audio_bitrate: String = row.get(6).unwrap_or_else(|_| "320k".to_string());
        let language: String = row.get(7).unwrap_or_else(|_| "ru".to_string());
        let send_as_document: i32 = row.get(8).unwrap_or(0);
        let send_audio_as_document: i32 = row.get(9).unwrap_or(0);
        let subscription_expires_at: Option<String> = row.get(10)?;
        let telegram_charge_id: Option<String> = row.get(11)?;
        let is_recurring: bool = row.get::<_, i32>(12).unwrap_or(0) != 0;
        let burn_subtitles: i32 = row.get(13).unwrap_or(0);
        let progress_bar_style: String = row.get(14).unwrap_or_else(|_| "classic".to_string());
        let is_blocked: bool = row.get::<_, i32>(15).unwrap_or(0) != 0;

        Ok(Some(User {
            telegram_id,
            username,
            plan,
            download_format,
            download_subtitles,
            video_quality,
            audio_bitrate,
            language,
            send_as_document,
            send_audio_as_document,
            subscription_expires_at,
            telegram_charge_id,
            is_recurring,
            burn_subtitles,
            progress_bar_style,
            is_blocked,
        }))
    } else {
        Ok(None)
    }
}

/// Updates the user's plan.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `plan` - New user plan (e.g. "free", "premium")
///
/// # Returns
///
/// Returns `Ok(())` on success or a database error.
pub fn update_user_plan(conn: &DbConnection, telegram_id: i64, plan: &str) -> Result<()> {
    if plan == "free" {
        conn.execute(
            "INSERT INTO subscriptions (user_id, plan, expires_at, telegram_charge_id, is_recurring)
             VALUES (?1, ?2, NULL, NULL, 0)
             ON CONFLICT(user_id) DO UPDATE SET
                plan = excluded.plan,
                expires_at = NULL,
                telegram_charge_id = NULL,
                is_recurring = 0,
                updated_at = CURRENT_TIMESTAMP",
            [&telegram_id as &dyn rusqlite::ToSql, &plan as &dyn rusqlite::ToSql],
        )?;
    } else {
        conn.execute(
            "INSERT INTO subscriptions (user_id, plan)
             VALUES (?1, ?2)
             ON CONFLICT(user_id) DO UPDATE SET
                plan = excluded.plan,
                updated_at = CURRENT_TIMESTAMP",
            [&telegram_id as &dyn rusqlite::ToSql, &plan as &dyn rusqlite::ToSql],
        )?;
    }
    conn.execute(
        "UPDATE users SET plan = ?1 WHERE telegram_id = ?2",
        [&plan as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Updates the user's plan and sets the subscription expiry date.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `plan` - New user plan (e.g. "free", "premium", "vip")
/// * `days` - Number of days the subscription is valid (None for unlimited/free)
///
/// # Returns
///
/// Returns `Ok(())` on success or a database error.
pub fn update_user_plan_with_expiry(
    conn: &DbConnection,
    telegram_id: i64,
    plan: &str,
    days: Option<i32>,
) -> Result<()> {
    if let Some(days_count) = days {
        // Set expiry date N days from now
        conn.execute(
            "INSERT INTO subscriptions (user_id, plan, expires_at)
             VALUES (?1, ?2, datetime('now', '+' || ?3 || ' days'))
             ON CONFLICT(user_id) DO UPDATE SET
                plan = excluded.plan,
                expires_at = excluded.expires_at,
                updated_at = CURRENT_TIMESTAMP",
            [
                &telegram_id as &dyn rusqlite::ToSql,
                &plan as &dyn rusqlite::ToSql,
                &days_count as &dyn rusqlite::ToSql,
            ],
        )?;
        conn.execute(
            "UPDATE users SET plan = ?1 WHERE telegram_id = ?2",
            [&plan as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
        )?;
    } else {
        // For free plan or unlimited subscriptions, clear expiry date
        conn.execute(
            "INSERT INTO subscriptions (user_id, plan, expires_at)
             VALUES (?1, ?2, NULL)
             ON CONFLICT(user_id) DO UPDATE SET
                plan = excluded.plan,
                expires_at = NULL,
                updated_at = CURRENT_TIMESTAMP",
            [&telegram_id as &dyn rusqlite::ToSql, &plan as &dyn rusqlite::ToSql],
        )?;
        conn.execute(
            "UPDATE users SET plan = ?1 WHERE telegram_id = ?2",
            [&plan as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
        )?;
    }
    Ok(())
}

/// Checks and updates expired subscriptions by downgrading them to free.
///
/// # Arguments
///
/// * `conn` - Database connection
///
/// # Returns
///
/// Returns the number of updated users.
pub fn expire_old_subscriptions(conn: &DbConnection) -> Result<usize> {
    let expired_user_ids = {
        let mut stmt = conn.prepare(
            "SELECT user_id FROM subscriptions
             WHERE expires_at IS NOT NULL
             AND expires_at < datetime('now', 'utc')
             AND plan != 'free'",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;

        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        ids
    };

    if expired_user_ids.is_empty() {
        return Ok(0);
    }

    conn.execute(
        "UPDATE subscriptions
         SET plan = 'free',
             expires_at = NULL,
             telegram_charge_id = NULL,
             is_recurring = 0,
             updated_at = CURRENT_TIMESTAMP
         WHERE user_id IN (
             SELECT user_id FROM subscriptions
             WHERE expires_at IS NOT NULL
               AND expires_at < datetime('now', 'utc')
               AND plan != 'free'
         )",
        [],
    )?;

    for user_id in &expired_user_ids {
        conn.execute("UPDATE users SET plan = 'free' WHERE telegram_id = ?1", [user_id])?;
    }

    if !expired_user_ids.is_empty() {
        log::info!("Expired {} subscription(s)", expired_user_ids.len());
    }

    Ok(expired_user_ids.len())
}

/// Logs a user request into the request history.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `user_id` - Telegram ID of the user
/// * `request_text` - Request text (usually a URL)
///
/// # Returns
///
/// Returns `Ok(())` on success or a database error.
pub fn log_request(conn: &DbConnection, user_id: i64, request_text: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO request_history (user_id, request_text) VALUES (?1, ?2)",
        [&user_id as &dyn rusqlite::ToSql, &request_text as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Gets the preferred download format of the user.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
///
/// # Returns
///
/// Returns the download format ("mp3", "mp4", "srt", "txt") or "mp3" by default
/// if the user is not found or an error occurred.
pub fn get_user_download_format(conn: &DbConnection, telegram_id: i64) -> Result<String> {
    let mut stmt = conn.prepare("SELECT download_format FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(row.get(0)?)
    } else {
        Ok("mp3".to_string())
    }
}

/// Sets the preferred download format of the user.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `format` - Download format: "mp3", "mp4", "srt", "txt"
///
/// # Returns
///
/// Returns `Ok(())` on success or a database error.
pub fn set_user_download_format(conn: &DbConnection, telegram_id: i64, format: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET download_format = ?1 WHERE telegram_id = ?2",
        [&format as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Gets the subtitle download setting of the user.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
///
/// # Returns
///
/// Returns `true` if subtitle download is enabled, `false` if disabled or user not found.
pub fn get_user_download_subtitles(conn: &DbConnection, telegram_id: i64) -> Result<bool> {
    let mut stmt = conn.prepare("SELECT download_subtitles FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        let subtitles: i32 = row.get(0)?;
        Ok(subtitles == 1)
    } else {
        Ok(false)
    }
}

/// Sets the subtitle download setting of the user.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `enabled` - Enable (`true`) or disable (`false`) subtitle download
///
/// # Returns
///
/// Returns `Ok(())` on success or a database error.
pub fn set_user_download_subtitles(conn: &DbConnection, telegram_id: i64, enabled: bool) -> Result<()> {
    let value = if enabled { 1 } else { 0 };
    conn.execute(
        "UPDATE users SET download_subtitles = ?1 WHERE telegram_id = ?2",
        [&value as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Gets the subtitle burn-in setting for video.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
///
/// # Returns
///
/// Returns `Ok(true)` if burn-in is enabled, `Ok(false)` if disabled,
/// or a database error.
pub fn get_user_burn_subtitles(conn: &DbConnection, telegram_id: i64) -> Result<bool> {
    let mut stmt = conn.prepare("SELECT COALESCE(burn_subtitles, 0) FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        let burn_subtitles: i32 = row.get(0)?;
        Ok(burn_subtitles == 1)
    } else {
        Ok(false)
    }
}

/// Sets the subtitle burn-in setting for video.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `enabled` - Enable (`true`) or disable (`false`) subtitle burn-in
///
/// # Returns
///
/// Returns `Ok(())` on success or a database error.
pub fn set_user_burn_subtitles(conn: &DbConnection, telegram_id: i64, enabled: bool) -> Result<()> {
    let value = if enabled { 1 } else { 0 };
    conn.execute(
        "UPDATE users SET burn_subtitles = ?1 WHERE telegram_id = ?2",
        [&value as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

// ==================== Subtitle Style Settings ====================

/// User's subtitle style preferences for burned subtitles.
#[derive(Debug, Clone)]
pub struct SubtitleStyle {
    pub font_size: String,
    pub text_color: String,
    pub outline_color: String,
    pub outline_width: i32,
    pub shadow: i32,
    pub position: String,
}

impl Default for SubtitleStyle {
    fn default() -> Self {
        Self {
            font_size: "medium".to_string(),
            text_color: "white".to_string(),
            outline_color: "black".to_string(),
            outline_width: 2,
            shadow: 1,
            position: "bottom".to_string(),
        }
    }
}

impl SubtitleStyle {
    /// Builds the ffmpeg force_style string from subtitle settings.
    pub fn to_force_style(&self) -> String {
        let font_size = match self.font_size.as_str() {
            "small" => 16,
            "medium" => 24,
            "large" => 32,
            "xlarge" => 40,
            _ => 24,
        };

        let primary_colour = match self.text_color.as_str() {
            "white" => "&H00FFFFFF",
            "yellow" => "&H0000FFFF",
            "cyan" => "&H00FFFF00",
            "green" => "&H0000FF00",
            _ => "&H00FFFFFF",
        };

        let outline_colour = match self.outline_color.as_str() {
            "black" => "&H00000000",
            "dark_gray" => "&H00404040",
            "none" => "&HFF000000",
            _ => "&H00000000",
        };

        // ASS Alignment: bottom-center=2, top-center=8
        let alignment = match self.position.as_str() {
            "top" => 8,
            _ => 2,
        };

        format!(
            "FontName=DejaVu Sans,FontSize={},PrimaryColour={},OutlineColour={},Outline={},Shadow={},Alignment={}",
            font_size, primary_colour, outline_colour, self.outline_width, self.shadow, alignment
        )
    }
}

/// Gets the subtitle style settings for a user.
pub fn get_user_subtitle_style(conn: &DbConnection, telegram_id: i64) -> Result<SubtitleStyle> {
    let mut stmt = conn.prepare(
        "SELECT subtitle_font_size, subtitle_text_color, subtitle_outline_color, \
         subtitle_outline_width, subtitle_shadow, subtitle_position \
         FROM users WHERE telegram_id = ?",
    )?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(SubtitleStyle {
            font_size: row.get(0).unwrap_or_else(|_| "medium".to_string()),
            text_color: row.get(1).unwrap_or_else(|_| "white".to_string()),
            outline_color: row.get(2).unwrap_or_else(|_| "black".to_string()),
            outline_width: row.get(3).unwrap_or(2),
            shadow: row.get(4).unwrap_or(1),
            position: row.get(5).unwrap_or_else(|_| "bottom".to_string()),
        })
    } else {
        Ok(SubtitleStyle::default())
    }
}

pub fn set_user_subtitle_font_size(conn: &DbConnection, telegram_id: i64, value: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET subtitle_font_size = ?1 WHERE telegram_id = ?2",
        [&value as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

pub fn set_user_subtitle_text_color(conn: &DbConnection, telegram_id: i64, value: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET subtitle_text_color = ?1 WHERE telegram_id = ?2",
        [&value as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

pub fn set_user_subtitle_outline_color(conn: &DbConnection, telegram_id: i64, value: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET subtitle_outline_color = ?1 WHERE telegram_id = ?2",
        [&value as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

pub fn set_user_subtitle_outline_width(conn: &DbConnection, telegram_id: i64, value: i32) -> Result<()> {
    conn.execute(
        "UPDATE users SET subtitle_outline_width = ?1 WHERE telegram_id = ?2",
        [&value as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

pub fn set_user_subtitle_shadow(conn: &DbConnection, telegram_id: i64, value: i32) -> Result<()> {
    conn.execute(
        "UPDATE users SET subtitle_shadow = ?1 WHERE telegram_id = ?2",
        [&value as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

pub fn set_user_subtitle_position(conn: &DbConnection, telegram_id: i64, value: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET subtitle_position = ?1 WHERE telegram_id = ?2",
        [&value as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Gets the progress bar style of the user.
pub fn get_user_progress_bar_style(conn: &DbConnection, telegram_id: i64) -> Result<String> {
    let mut stmt = conn.prepare("SELECT progress_bar_style FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(row.get(0).unwrap_or_else(|_| "classic".to_string()))
    } else {
        Ok("classic".to_string())
    }
}

/// Sets the progress bar style of the user.
pub fn set_user_progress_bar_style(conn: &DbConnection, telegram_id: i64, style: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET progress_bar_style = ?1 WHERE telegram_id = ?2",
        [&style as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Gets the video quality setting of the user.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
///
/// # Returns
///
/// Returns the video quality ("best", "1080p", "720p", "480p", "360p") or "best" by default.
pub fn get_user_video_quality(conn: &DbConnection, telegram_id: i64) -> Result<String> {
    let mut stmt = conn.prepare("SELECT video_quality FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(row.get(0).unwrap_or_else(|_| "best".to_string()))
    } else {
        Ok("best".to_string())
    }
}

/// Sets the video quality setting of the user.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `quality` - Video quality: "best", "1080p", "720p", "480p", "360p"
///
/// # Returns
///
/// Returns `Ok(())` on success or a database error.
pub fn set_user_video_quality(conn: &DbConnection, telegram_id: i64, quality: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET video_quality = ?1 WHERE telegram_id = ?2",
        [&quality as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Gets the video send type for the user (0 = Media, 1 = Document).
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
///
/// # Returns
///
/// Returns `Ok(0)` for Media (send_video) or `Ok(1)` for Document (send_document).
/// Defaults to 0 (Media).
pub fn get_user_send_as_document(conn: &DbConnection, telegram_id: i64) -> Result<i32> {
    let mut stmt = conn.prepare("SELECT send_as_document FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(row.get(0).unwrap_or(0))
    } else {
        Ok(0) // Default to Media
    }
}

/// Sets the video send type for the user.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `send_as_document` - 0 = Media (send_video), 1 = Document (send_document)
///
/// # Returns
///
/// Returns `Ok(())` on success or a database error.
pub fn set_user_send_as_document(conn: &DbConnection, telegram_id: i64, send_as_document: i32) -> Result<()> {
    conn.execute(
        "UPDATE users SET send_as_document = ?1 WHERE telegram_id = ?2",
        [
            &send_as_document as &dyn rusqlite::ToSql,
            &telegram_id as &dyn rusqlite::ToSql,
        ],
    )?;
    Ok(())
}

/// Gets the audio send type for the user (0 = Media, 1 = Document).
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
///
/// # Returns
///
/// Returns `Ok(0)` for Media (send_audio) or `Ok(1)` for Document (send_document).
/// Defaults to 0 (Media).
pub fn get_user_send_audio_as_document(conn: &DbConnection, telegram_id: i64) -> Result<i32> {
    let mut stmt = conn.prepare("SELECT send_audio_as_document FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(row.get(0).unwrap_or(0))
    } else {
        Ok(0) // Default to Media
    }
}

/// Sets the audio send type for the user.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `send_audio_as_document` - 0 = Media (send_audio), 1 = Document (send_document)
///
/// # Returns
///
/// Returns `Ok(())` on success or a database error.
pub fn set_user_send_audio_as_document(
    conn: &DbConnection,
    telegram_id: i64,
    send_audio_as_document: i32,
) -> Result<()> {
    conn.execute(
        "UPDATE users SET send_audio_as_document = ?1 WHERE telegram_id = ?2",
        [
            &send_audio_as_document as &dyn rusqlite::ToSql,
            &telegram_id as &dyn rusqlite::ToSql,
        ],
    )?;
    Ok(())
}

/// Gets the audio bitrate setting of the user.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
///
/// # Returns
///
/// Returns the audio bitrate ("128k", "192k", "256k", "320k") or "320k" by default.
pub fn get_user_audio_bitrate(conn: &DbConnection, telegram_id: i64) -> Result<String> {
    let mut stmt = conn.prepare("SELECT audio_bitrate FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(row.get(0).unwrap_or_else(|_| "320k".to_string()))
    } else {
        Ok("320k".to_string())
    }
}

/// Sets the audio bitrate setting of the user.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `bitrate` - Audio bitrate: "128k", "192k", "256k", "320k"
///
/// # Returns
///
/// Returns `Ok(())` on success or a database error.
pub fn set_user_audio_bitrate(conn: &DbConnection, telegram_id: i64, bitrate: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET audio_bitrate = ?1 WHERE telegram_id = ?2",
        [&bitrate as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Gets the preferred language of the user (IETF code, e.g. "en", "ru").
pub fn get_user_language(conn: &DbConnection, telegram_id: i64) -> Result<String> {
    let mut stmt = conn.prepare("SELECT language FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(row.get(0).unwrap_or_else(|_| "ru".to_string()))
    } else {
        Ok("ru".to_string())
    }
}

/// Sets the preferred language of the user.
pub fn set_user_language(conn: &DbConnection, telegram_id: i64, language: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET language = ?1 WHERE telegram_id = ?2",
        [&language as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Returns total user counts grouped by plan and blocked status.
pub fn get_user_counts(conn: &DbConnection) -> Result<UserCounts> {
    let mut counts = UserCounts::default();
    let mut stmt = conn.prepare(
        "SELECT COALESCE(s.plan, u.plan) as plan, COALESCE(u.is_blocked, 0) as is_blocked, COUNT(*)
         FROM users u
         LEFT JOIN subscriptions s ON s.user_id = u.telegram_id
         GROUP BY plan, is_blocked",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i32>(1)?, row.get::<_, usize>(2)?))
    })?;
    for row in rows {
        let (plan, blocked, count) = row?;
        counts.total += count;
        if blocked != 0 {
            counts.blocked += count;
        }
        match plan.as_str() {
            "premium" => counts.premium += count,
            "vip" => counts.vip += count,
            _ => counts.free += count,
        }
    }
    Ok(counts)
}

/// Aggregated user counts.
#[derive(Debug, Default)]
pub struct UserCounts {
    pub total: usize,
    pub free: usize,
    pub premium: usize,
    pub vip: usize,
    pub blocked: usize,
}

/// Returns a paginated, filtered list of users.
///
/// `filter` values: `None` = all, `"free"`, `"premium"`, `"vip"`, `"blocked"`.
pub fn get_users_paginated(
    conn: &DbConnection,
    filter: Option<&str>,
    offset: usize,
    limit: usize,
) -> Result<(Vec<User>, usize)> {
    // Build WHERE clause
    let (where_clause, param): (&str, Option<&dyn rusqlite::ToSql>) = match filter {
        Some("free") => ("WHERE COALESCE(s.plan, u.plan) = 'free'", None),
        Some("premium") => ("WHERE COALESCE(s.plan, u.plan) = 'premium'", None),
        Some("vip") => ("WHERE COALESCE(s.plan, u.plan) = 'vip'", None),
        Some("blocked") => ("WHERE COALESCE(u.is_blocked, 0) = 1", None),
        _ => ("", None),
    };
    let _ = param; // unused, filters are literals

    // Count total matching
    let count_sql = format!(
        "SELECT COUNT(*) FROM users u LEFT JOIN subscriptions s ON s.user_id = u.telegram_id {}",
        where_clause
    );
    let total: usize = conn.query_row(&count_sql, [], |row| row.get(0))?;

    // Fetch page
    let query_sql = format!(
        "SELECT
            u.telegram_id,
            u.username,
            COALESCE(s.plan, u.plan) as plan,
            u.download_format,
            u.download_subtitles,
            u.video_quality,
            u.audio_bitrate,
            u.language,
            u.send_as_document,
            u.send_audio_as_document,
            s.expires_at as subscription_expires_at,
            s.telegram_charge_id as telegram_charge_id,
            COALESCE(s.is_recurring, 0) as is_recurring,
            COALESCE(u.burn_subtitles, 0) as burn_subtitles,
            COALESCE(u.progress_bar_style, 'classic') as progress_bar_style,
            COALESCE(u.is_blocked, 0) as is_blocked
        FROM users u
        LEFT JOIN subscriptions s ON s.user_id = u.telegram_id
        {}
        ORDER BY u.telegram_id
        LIMIT ?1 OFFSET ?2",
        where_clause
    );
    let mut stmt = conn.prepare(&query_sql)?;
    let rows = stmt.query_map(
        [&limit as &dyn rusqlite::ToSql, &offset as &dyn rusqlite::ToSql],
        |row| {
            Ok(User {
                telegram_id: row.get(0)?,
                username: row.get(1)?,
                plan: row.get(2)?,
                download_format: row.get(3)?,
                download_subtitles: row.get(4)?,
                video_quality: row.get(5).unwrap_or_else(|_| "best".to_string()),
                audio_bitrate: row.get(6).unwrap_or_else(|_| "320k".to_string()),
                language: row.get(7).unwrap_or_else(|_| "ru".to_string()),
                send_as_document: row.get(8).unwrap_or(0),
                send_audio_as_document: row.get(9).unwrap_or(0),
                subscription_expires_at: row.get(10)?,
                telegram_charge_id: row.get(11)?,
                is_recurring: row.get::<_, i32>(12).unwrap_or(0) != 0,
                burn_subtitles: row.get(13).unwrap_or(0),
                progress_bar_style: row.get(14).unwrap_or_else(|_| "classic".to_string()),
                is_blocked: row.get::<_, i32>(15).unwrap_or(0) != 0,
            })
        },
    )?;

    let mut users = Vec::new();
    for row in rows {
        users.push(row?);
    }
    Ok((users, total))
}

/// Checks if a user is blocked.
pub fn is_user_blocked(conn: &DbConnection, telegram_id: i64) -> Result<bool> {
    let blocked: i32 = conn
        .query_row(
            "SELECT COALESCE(is_blocked, 0) FROM users WHERE telegram_id = ?",
            [telegram_id],
            |row| row.get(0),
        )
        .unwrap_or(0);
    Ok(blocked != 0)
}

/// Sets the blocked status of a user.
pub fn set_user_blocked(conn: &DbConnection, telegram_id: i64, blocked: bool) -> Result<()> {
    conn.execute(
        "UPDATE users SET is_blocked = ?1 WHERE telegram_id = ?2",
        [
            &(blocked as i32) as &dyn rusqlite::ToSql,
            &telegram_id as &dyn rusqlite::ToSql,
        ],
    )?;
    Ok(())
}

/// Searches users by telegram_id or username (partial match).
pub fn search_users(conn: &DbConnection, query: &str) -> Result<Vec<User>> {
    let pattern = format!("%{}%", query);
    let mut stmt = conn.prepare(
        "SELECT
            u.telegram_id,
            u.username,
            COALESCE(s.plan, u.plan) as plan,
            u.download_format,
            u.download_subtitles,
            u.video_quality,
            u.audio_bitrate,
            u.language,
            u.send_as_document,
            u.send_audio_as_document,
            s.expires_at as subscription_expires_at,
            s.telegram_charge_id as telegram_charge_id,
            COALESCE(s.is_recurring, 0) as is_recurring,
            COALESCE(u.burn_subtitles, 0) as burn_subtitles,
            COALESCE(u.progress_bar_style, 'classic') as progress_bar_style,
            COALESCE(u.is_blocked, 0) as is_blocked
        FROM users u
        LEFT JOIN subscriptions s ON s.user_id = u.telegram_id
        WHERE CAST(u.telegram_id AS TEXT) LIKE ?1 OR COALESCE(u.username, '') LIKE ?1
        ORDER BY u.telegram_id
        LIMIT 20",
    )?;
    let rows = stmt.query_map([&pattern as &dyn rusqlite::ToSql], |row| {
        Ok(User {
            telegram_id: row.get(0)?,
            username: row.get(1)?,
            plan: row.get(2)?,
            download_format: row.get(3)?,
            download_subtitles: row.get(4)?,
            video_quality: row.get(5).unwrap_or_else(|_| "best".to_string()),
            audio_bitrate: row.get(6).unwrap_or_else(|_| "320k".to_string()),
            language: row.get(7).unwrap_or_else(|_| "ru".to_string()),
            send_as_document: row.get(8).unwrap_or(0),
            send_audio_as_document: row.get(9).unwrap_or(0),
            subscription_expires_at: row.get(10)?,
            telegram_charge_id: row.get(11)?,
            is_recurring: row.get::<_, i32>(12).unwrap_or(0) != 0,
            burn_subtitles: row.get(13).unwrap_or(0),
            progress_bar_style: row.get(14).unwrap_or_else(|_| "classic".to_string()),
            is_blocked: row.get::<_, i32>(15).unwrap_or(0) != 0,
        })
    })?;

    let mut users = Vec::new();
    for row in rows {
        users.push(row?);
    }
    Ok(users)
}

/// Structure representing a download history entry.
#[derive(Debug, Clone)]
pub struct DownloadHistoryEntry {
    /// Record ID
    pub id: i64,
    /// URL of the downloaded content
    pub url: String,
    /// Track/video title
    pub title: String,
    /// Download format (mp3, mp4, srt, txt)
    pub format: String,
    /// Download date and time
    pub downloaded_at: String,
    /// Telegram file_id (optional)
    pub file_id: Option<String>,
    /// Track/video author (optional)
    pub author: Option<String>,
    /// File size in bytes (optional)
    pub file_size: Option<i64>,
    /// Duration in seconds (optional)
    pub duration: Option<i64>,
    /// Video quality (optional, for mp4)
    pub video_quality: Option<String>,
    /// Audio bitrate (optional, for mp3)
    pub audio_bitrate: Option<String>,
    /// Bot API base URL used when saving this entry (optional, for debugging)
    pub bot_api_url: Option<String>,
    /// Whether a local Bot API server was used (0/1, optional for older rows)
    pub bot_api_is_local: Option<i64>,
    /// Source file ID (for split videos)
    pub source_id: Option<i64>,
    /// Part number (for split videos)
    pub part_index: Option<i32>,
    /// User-defined category name (optional)
    pub category: Option<String>,
}

fn current_bot_api_info() -> (Option<String>, i64) {
    let url = std::env::var("BOT_API_URL").ok();
    let is_local = url.as_deref().map(|u| !u.contains("api.telegram.org")).unwrap_or(false);
    (url, if is_local { 1 } else { 0 })
}

/// Saves an entry to the download history.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `url` - URL of the downloaded content
/// * `title` - Track/video title
/// * `format` - Download format (mp3, mp4, srt, txt)
/// * `file_id` - Telegram file_id, if content was sent to Telegram (optional)
/// * `author` - Track/video author (optional)
/// * `file_size` - File size in bytes (optional)
/// * `duration` - Duration in seconds (optional)
/// * `video_quality` - Video quality (optional)
/// * `audio_bitrate` - Audio bitrate (optional)
/// * `source_id` - Source file ID (for split videos)
/// * `part_index` - Part number (for split videos)
///
/// # Returns
///
/// Returns `Ok(id)` on success (ID of the inserted record) or a database error.
pub fn save_download_history(
    conn: &DbConnection,
    telegram_id: i64,
    url: &str,
    title: &str,
    format: &str,
    file_id: Option<&str>,
    author: Option<&str>,
    file_size: Option<i64>,
    duration: Option<i64>,
    video_quality: Option<&str>,
    audio_bitrate: Option<&str>,
    source_id: Option<i64>,
    part_index: Option<i32>,
) -> Result<i64> {
    let (bot_api_url, bot_api_is_local) = current_bot_api_info();
    conn.execute(
        "INSERT INTO download_history (
            user_id, url, title, format, file_id, author, file_size, duration, video_quality, audio_bitrate,
            bot_api_url, bot_api_is_local, source_id, part_index
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        rusqlite::params![
            telegram_id,
            url,
            title,
            format,
            file_id,
            author,
            file_size,
            duration,
            video_quality,
            audio_bitrate,
            bot_api_url,
            bot_api_is_local,
            source_id,
            part_index
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Finds a cached Telegram file_id for the given URL, format and quality/bitrate.
/// Searches across ALL users — file_ids are reusable within the same Bot API server.
/// Returns the most recent file_id that matches.
pub fn find_cached_file_id(
    conn: &DbConnection,
    url: &str,
    format: &str,
    video_quality: Option<&str>,
    audio_bitrate: Option<&str>,
) -> Result<Option<String>> {
    let (current_api_url, current_is_local) = current_bot_api_info();
    let mut stmt = conn.prepare(
        "SELECT file_id FROM download_history
         WHERE url = ?1 AND format = ?2 AND file_id IS NOT NULL
         AND bot_api_is_local = ?3
         AND (?4 IS NULL OR video_quality = ?4)
         AND (?5 IS NULL OR audio_bitrate = ?5)
         AND (?6 IS NULL OR bot_api_url = ?6)
         ORDER BY downloaded_at DESC LIMIT 1",
    )?;
    let result = stmt.query_row(
        rusqlite::params![
            url,
            format,
            current_is_local,
            video_quality,
            audio_bitrate,
            current_api_url
        ],
        |row| row.get::<_, String>(0),
    );
    match result {
        Ok(fid) => Ok(Some(fid)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Gets the last N download history entries for a user.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `limit` - Maximum number of records (default 20)
///
/// # Returns
///
/// Returns `Ok(Vec<DownloadHistoryEntry>)` with history records or a database error.
pub fn get_download_history(
    conn: &DbConnection,
    telegram_id: i64,
    limit: Option<i32>,
) -> Result<Vec<DownloadHistoryEntry>> {
    let limit = limit.unwrap_or(20);
    let mut stmt = conn.prepare(
        "SELECT id, url, title, format, downloaded_at, file_id, author, file_size, duration, video_quality, audio_bitrate,
                bot_api_url, bot_api_is_local, source_id, part_index, category
         FROM download_history
         WHERE user_id = ? ORDER BY downloaded_at DESC LIMIT ?",
    )?;
    let rows = stmt.query_map(rusqlite::params![telegram_id, limit], |row| {
        Ok(DownloadHistoryEntry {
            id: row.get(0)?,
            url: row.get(1)?,
            title: row.get(2)?,
            format: row.get(3)?,
            downloaded_at: row.get(4)?,
            file_id: row.get(5)?,
            author: row.get(6)?,
            file_size: row.get(7)?,
            duration: row.get(8)?,
            video_quality: row.get(9)?,
            audio_bitrate: row.get(10)?,
            bot_api_url: row.get(11)?,
            bot_api_is_local: row.get(12)?,
            source_id: row.get(13)?,
            part_index: row.get(14)?,
            category: row.get(15)?,
        })
    })?;

    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }
    Ok(entries)
}

/// Structure representing a file with file_id for the administrator.
#[derive(Debug, Clone)]
pub struct SentFile {
    /// Record ID
    pub id: i64,
    /// Telegram ID of the user
    pub user_id: i64,
    /// Username of the user (if available)
    pub username: Option<String>,
    /// URL of the downloaded content
    pub url: String,
    /// File title
    pub title: String,
    /// File format (mp3, mp4, srt, txt)
    pub format: String,
    /// Download date and time
    pub downloaded_at: String,
    /// Telegram file_id
    pub file_id: String,
    /// Telegram message_id (for MTProto refresh)
    pub message_id: Option<i32>,
    /// Chat ID where message was sent
    pub chat_id: Option<i64>,
}

/// Gets the list of files with file_id for the administrator.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `limit` - Maximum number of records (default 50)
///
/// # Returns
///
/// Returns `Ok(Vec<SentFile>)` with file records or a database error.
/// Returns only files that have a file_id.
pub fn get_sent_files(conn: &DbConnection, limit: Option<i32>) -> Result<Vec<SentFile>> {
    let limit = limit.unwrap_or(50);
    let mut stmt = conn.prepare(
        "SELECT dh.id, dh.user_id, u.username, dh.url, dh.title, dh.format, dh.downloaded_at, dh.file_id,
                dh.message_id, dh.chat_id
         FROM download_history dh
         LEFT JOIN users u ON dh.user_id = u.telegram_id
         WHERE dh.file_id IS NOT NULL
         ORDER BY dh.downloaded_at DESC
         LIMIT ?",
    )?;
    let rows = stmt.query_map([limit], |row| {
        Ok(SentFile {
            id: row.get(0)?,
            user_id: row.get(1)?,
            username: row.get(2)?,
            url: row.get(3)?,
            title: row.get(4)?,
            format: row.get(5)?,
            downloaded_at: row.get(6)?,
            file_id: row.get(7)?,
            message_id: row.get(8)?,
            chat_id: row.get(9)?,
        })
    })?;

    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }
    Ok(entries)
}

/// Deletes an entry from the download history.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `entry_id` - ID of the record to delete
///
/// # Returns
///
/// Returns `Ok(true)` if the record was deleted, `Ok(false)` if not found,
/// or a database error.
pub fn delete_download_history_entry(conn: &DbConnection, telegram_id: i64, entry_id: i64) -> Result<bool> {
    let rows_affected = conn.execute(
        "DELETE FROM download_history WHERE id = ?1 AND user_id = ?2",
        [&entry_id as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(rows_affected > 0)
}

/// Gets a download history entry by ID.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `entry_id` - Record ID
///
/// # Returns
///
/// Returns `Ok(Some(DownloadHistoryEntry))` if found, `Ok(None)` if not found,
/// or a database error.
pub fn get_download_history_entry(
    conn: &DbConnection,
    telegram_id: i64,
    entry_id: i64,
) -> Result<Option<DownloadHistoryEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, url, title, format, downloaded_at, file_id, author, file_size, duration, video_quality, audio_bitrate,
                bot_api_url, bot_api_is_local, source_id, part_index, category
         FROM download_history
         WHERE id = ?1 AND user_id = ?2",
    )?;
    let mut rows = stmt.query_map(rusqlite::params![entry_id, telegram_id], |row| {
        Ok(DownloadHistoryEntry {
            id: row.get(0)?,
            url: row.get(1)?,
            title: row.get(2)?,
            format: row.get(3)?,
            downloaded_at: row.get(4)?,
            file_id: row.get(5)?,
            author: row.get(6)?,
            file_size: row.get(7)?,
            duration: row.get(8)?,
            video_quality: row.get(9)?,
            audio_bitrate: row.get(10)?,
            bot_api_url: row.get(11)?,
            bot_api_is_local: row.get(12)?,
            source_id: row.get(13)?,
            part_index: row.get(14)?,
            category: row.get(15)?,
        })
    })?;

    if let Some(row) = rows.next() {
        Ok(Some(row?))
    } else {
        Ok(None)
    }
}

/// User statistics structure
#[derive(Debug, Clone)]
pub struct UserStats {
    pub total_downloads: i64,
    pub total_size: i64, // in bytes (approximate)
    pub active_days: i64,
    pub top_artists: Vec<(String, i64)>,     // (artist, count)
    pub top_formats: Vec<(String, i64)>,     // (format, count)
    pub activity_by_day: Vec<(String, i64)>, // (date, count) for the last 7 days
}

/// Gets user statistics
pub fn get_user_stats(conn: &DbConnection, telegram_id: i64) -> Result<UserStats> {
    // Total download count
    let total_downloads: i64 = conn.query_row(
        "SELECT COUNT(*) FROM download_history WHERE user_id = ?",
        [&telegram_id as &dyn rusqlite::ToSql],
        |row| row.get(0),
    )?;

    // Approximate total size (rough estimate: mp3 ~5MB, mp4 ~50MB)
    let total_size: i64 = match conn.query_row(
        "SELECT
            SUM(CASE
                WHEN format = 'mp3' THEN 5000000
                WHEN format = 'mp4' THEN 50000000
                ELSE 1000000
            END)
        FROM download_history WHERE user_id = ?",
        [&telegram_id as &dyn rusqlite::ToSql],
        |row| row.get::<_, Option<i64>>(0),
    ) {
        Ok(Some(size)) => size,
        Ok(None) => 0,
        Err(e) => return Err(e),
    };

    // Number of active days
    let active_days: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT DATE(downloaded_at)) FROM download_history WHERE user_id = ?",
        [&telegram_id as &dyn rusqlite::ToSql],
        |row| row.get(0),
    )?;

    // Top-5 artists (parsed from title: "Artist - Song")
    let mut stmt =
        conn.prepare("SELECT title FROM download_history WHERE user_id = ? ORDER BY downloaded_at DESC LIMIT 100")?;
    let rows = stmt.query_map([&telegram_id as &dyn rusqlite::ToSql], |row| row.get::<_, String>(0))?;

    let mut artist_counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for row in rows {
        if let Ok(title) = row {
            // Try to extract artist from "Artist - Song" format
            if let Some(pos) = title.find(" - ") {
                let artist = title[..pos].trim().to_string();
                if !artist.is_empty() {
                    *artist_counts.entry(artist).or_insert(0) += 1;
                }
            }
        }
    }

    let mut top_artists: Vec<(String, i64)> = artist_counts.into_iter().collect();
    top_artists.sort_by(|a, b| b.1.cmp(&a.1));
    top_artists.truncate(5);

    // Top formats
    let mut stmt = conn.prepare(
        "SELECT format, COUNT(*) as cnt FROM download_history
         WHERE user_id = ? GROUP BY format ORDER BY cnt DESC LIMIT 5",
    )?;
    let rows = stmt.query_map([&telegram_id as &dyn rusqlite::ToSql], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;

    let mut top_formats = Vec::new();
    for row in rows {
        if let Ok((format, count)) = row {
            top_formats.push((format, count));
        }
    }

    // Activity by day (last 7 days)
    let mut stmt = conn.prepare(
        "SELECT DATE(downloaded_at) as day, COUNT(*) as cnt
         FROM download_history
         WHERE user_id = ? AND downloaded_at >= datetime('now', '-7 days')
         GROUP BY DATE(downloaded_at)
         ORDER BY day DESC",
    )?;
    let rows = stmt.query_map([&telegram_id as &dyn rusqlite::ToSql], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;

    let mut activity_by_day = Vec::new();
    for row in rows {
        if let Ok((day, count)) = row {
            activity_by_day.push((day, count));
        }
    }

    Ok(UserStats {
        total_downloads,
        total_size,
        active_days,
        top_artists,
        top_formats,
        activity_by_day,
    })
}

/// Global statistics structure
#[derive(Debug, Clone)]
pub struct GlobalStats {
    pub total_users: i64,
    pub total_downloads: i64,
    pub top_tracks: Vec<(String, i64)>,  // (title, count)
    pub top_formats: Vec<(String, i64)>, // (format, count)
}

/// Gets global bot statistics
pub fn get_global_stats(conn: &DbConnection) -> Result<GlobalStats> {
    // Total number of users
    let total_users: i64 = conn.query_row("SELECT COUNT(DISTINCT user_id) FROM download_history", [], |row| {
        row.get(0)
    })?;

    // Total number of downloads
    let total_downloads: i64 = conn.query_row("SELECT COUNT(*) FROM download_history", [], |row| row.get(0))?;

    // Top-10 tracks (by title)
    let mut stmt = conn.prepare(
        "SELECT title, COUNT(*) as cnt FROM download_history
         GROUP BY title ORDER BY cnt DESC LIMIT 10",
    )?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;

    let mut top_tracks = Vec::new();
    for row in rows {
        if let Ok((title, count)) = row {
            top_tracks.push((title, count));
        }
    }

    // Top formats
    let mut stmt = conn.prepare(
        "SELECT format, COUNT(*) as cnt FROM download_history
         GROUP BY format ORDER BY cnt DESC",
    )?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;

    let mut top_formats = Vec::new();
    for row in rows {
        if let Ok((format, count)) = row {
            top_formats.push((format, count));
        }
    }

    Ok(GlobalStats {
        total_users,
        total_downloads,
        top_tracks,
        top_formats,
    })
}

/// Gets all download history for a user for export
pub fn get_all_download_history(conn: &DbConnection, telegram_id: i64) -> Result<Vec<DownloadHistoryEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, url, title, format, downloaded_at, file_id, author, file_size, duration, video_quality, audio_bitrate,
                bot_api_url, bot_api_is_local, source_id, part_index, category
         FROM download_history
         WHERE user_id = ? ORDER BY downloaded_at DESC",
    )?;
    let rows = stmt.query_map(rusqlite::params![telegram_id], |row| {
        Ok(DownloadHistoryEntry {
            id: row.get(0)?,
            url: row.get(1)?,
            title: row.get(2)?,
            format: row.get(3)?,
            downloaded_at: row.get(4)?,
            file_id: row.get(5)?,
            author: row.get(6)?,
            file_size: row.get(7)?,
            duration: row.get(8)?,
            video_quality: row.get(9)?,
            audio_bitrate: row.get(10)?,
            bot_api_url: row.get(11)?,
            bot_api_is_local: row.get(12)?,
            source_id: row.get(13)?,
            part_index: row.get(14)?,
            category: row.get(15)?,
        })
    })?;

    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }
    Ok(entries)
}

/// Gets filtered download history for the /downloads command
///
/// Returns only files with file_id (successfully sent) and only mp3/mp4 (excluding subtitles).
/// Supports filtering by file type and searching by title/author.
pub fn get_download_history_filtered(
    conn: &DbConnection,
    user_id: i64,
    file_type_filter: Option<&str>,
    search_text: Option<&str>,
    category_filter: Option<&str>,
) -> Result<Vec<DownloadHistoryEntry>> {
    let mut query = String::from(
        "SELECT id, url, title, format, downloaded_at, file_id, author, file_size,
         duration, video_quality, audio_bitrate, bot_api_url, bot_api_is_local, source_id, part_index, category
         FROM download_history WHERE user_id = ?",
    );

    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];

    // Only show files with file_id (successfully sent files)
    query.push_str(" AND file_id IS NOT NULL");

    // Only show mp3/mp4 (exclude subtitles)
    query.push_str(" AND (format = 'mp3' OR format = 'mp4')");

    if let Some(ft) = file_type_filter {
        query.push_str(" AND format = ?");
        params.push(Box::new(ft.to_string()));
    }

    if let Some(search) = search_text {
        query.push_str(" AND (title LIKE ? OR author LIKE ?)");
        let search_pattern = format!("%{}%", search);
        params.push(Box::new(search_pattern.clone()));
        params.push(Box::new(search_pattern));
    }

    if let Some(cat) = category_filter {
        query.push_str(" AND category = ?");
        params.push(Box::new(cat.to_string()));
    }

    query.push_str(" ORDER BY downloaded_at DESC");

    let mut stmt = conn.prepare(&query)?;
    let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let downloads = stmt
        .query_map(params_refs.as_slice(), |row| {
            Ok(DownloadHistoryEntry {
                id: row.get(0)?,
                url: row.get(1)?,
                title: row.get(2)?,
                format: row.get(3)?,
                downloaded_at: row.get(4)?,
                file_id: row.get(5)?,
                author: row.get(6)?,
                file_size: row.get(7)?,
                duration: row.get(8)?,
                video_quality: row.get(9)?,
                audio_bitrate: row.get(10)?,
                bot_api_url: row.get(11)?,
                bot_api_is_local: row.get(12)?,
                source_id: row.get(13)?,
                part_index: row.get(14)?,
                category: row.get(15)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(downloads)
}

// ==================== User Categories ====================

/// Creates a user category (or ignores if it already exists).
pub fn create_user_category(conn: &DbConnection, user_id: i64, name: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO user_categories (user_id, name) VALUES (?1, ?2)",
        rusqlite::params![user_id, name],
    )?;
    Ok(())
}

/// Returns the user's category names ordered alphabetically.
pub fn get_user_categories(conn: &DbConnection, user_id: i64) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT name FROM user_categories WHERE user_id = ? ORDER BY name")?;
    let rows = stmt.query_map(rusqlite::params![user_id], |row| row.get(0))?;
    let mut cats = Vec::new();
    for row in rows {
        cats.push(row?);
    }
    Ok(cats)
}

/// Deletes a user category. Existing download assignments keep their text value but won't be filterable.
pub fn delete_user_category(conn: &DbConnection, user_id: i64, name: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM user_categories WHERE user_id = ?1 AND name = ?2",
        rusqlite::params![user_id, name],
    )?;
    Ok(())
}

/// Sets (or clears) the category on a download history entry.
pub fn set_download_category(
    conn: &DbConnection,
    user_id: i64,
    download_id: i64,
    category: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE download_history SET category = ?1 WHERE id = ?2 AND user_id = ?3",
        rusqlite::params![category, download_id, user_id],
    )?;
    Ok(())
}

// ==================== New Category Sessions ====================

/// Stores a new-category session: user is creating a category for a specific download.
pub fn create_new_category_session(conn: &DbConnection, user_id: i64, download_id: i64) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO new_category_sessions (user_id, download_id, created_at) VALUES (?1, ?2, datetime('now'))",
        rusqlite::params![user_id, download_id],
    )?;
    Ok(())
}

/// Returns the download_id for an active new-category session, or None.
pub fn get_active_new_category_session(conn: &DbConnection, user_id: i64) -> Result<Option<i64>> {
    let mut stmt = conn.prepare(
        "SELECT download_id FROM new_category_sessions WHERE user_id = ? AND created_at > datetime('now', '-10 minutes')",
    )?;
    let mut rows = stmt.query_map(rusqlite::params![user_id], |row| row.get(0))?;
    if let Some(row) = rows.next() {
        Ok(Some(row?))
    } else {
        Ok(None)
    }
}

/// Deletes the new-category session for a user.
pub fn delete_new_category_session(conn: &DbConnection, user_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM new_category_sessions WHERE user_id = ?",
        rusqlite::params![user_id],
    )?;
    Ok(())
}

/// Gets filtered cuts history for the /downloads command
pub fn get_cuts_history_filtered(
    conn: &DbConnection,
    user_id: i64,
    search_text: Option<&str>,
) -> Result<Vec<DownloadHistoryEntry>> {
    let mut query = String::from(
        "SELECT id, original_url, title, output_kind, created_at, file_id, file_size,
         duration, video_quality FROM cuts WHERE user_id = ?",
    );

    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];

    // Only show files with file_id
    query.push_str(" AND file_id IS NOT NULL");

    if let Some(search) = search_text {
        query.push_str(" AND title LIKE ?");
        let search_pattern = format!("%{}%", search);
        params.push(Box::new(search_pattern));
    }

    query.push_str(" ORDER BY created_at DESC");

    let mut stmt = conn.prepare(&query)?;
    let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let cuts = stmt
        .query_map(params_refs.as_slice(), |row| {
            Ok(DownloadHistoryEntry {
                id: row.get(0)?,
                url: row.get(1)?,
                title: row.get(2)?,
                format: String::from("edit"), // Marker for UI
                downloaded_at: row.get(4)?,
                file_id: row.get(5)?,
                author: None,
                file_size: row.get(6)?,
                duration: row.get(7)?,
                video_quality: row.get(8)?,
                audio_bitrate: None,
                bot_api_url: None,
                bot_api_is_local: None,
                source_id: None,
                part_index: None,
                category: None,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(cuts)
}

/// Gets the list of all users from the database.
///
/// # Arguments
///
/// * `conn` - Database connection
///
/// # Returns
///
/// Returns `Ok(Vec<User>)` with all users or a database error.
pub fn get_all_users(conn: &DbConnection) -> Result<Vec<User>> {
    let mut stmt = conn.prepare(
        "SELECT
            u.telegram_id,
            u.username,
            COALESCE(s.plan, u.plan) as plan,
            u.download_format,
            u.download_subtitles,
            u.video_quality,
            u.audio_bitrate,
            u.language,
            u.send_as_document,
            u.send_audio_as_document,
            s.expires_at as subscription_expires_at,
            s.telegram_charge_id as telegram_charge_id,
            COALESCE(s.is_recurring, 0) as is_recurring,
            COALESCE(u.burn_subtitles, 0) as burn_subtitles,
            COALESCE(u.progress_bar_style, 'classic') as progress_bar_style,
            COALESCE(u.is_blocked, 0) as is_blocked
        FROM users u
        LEFT JOIN subscriptions s ON s.user_id = u.telegram_id
        ORDER BY u.telegram_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(User {
            telegram_id: row.get(0)?,
            username: row.get(1)?,
            plan: row.get(2)?,
            download_format: row.get(3)?,
            download_subtitles: row.get(4)?,
            video_quality: row.get(5).unwrap_or_else(|_| "best".to_string()),
            audio_bitrate: row.get(6).unwrap_or_else(|_| "320k".to_string()),
            language: row.get(7).unwrap_or_else(|_| "ru".to_string()),
            send_as_document: row.get(8).unwrap_or(0),
            send_audio_as_document: row.get(9).unwrap_or(0),
            subscription_expires_at: row.get(10)?,
            telegram_charge_id: row.get(11)?,
            is_recurring: row.get::<_, i32>(12).unwrap_or(0) != 0,
            burn_subtitles: row.get(13).unwrap_or(0),
            progress_bar_style: row.get(14).unwrap_or_else(|_| "classic".to_string()),
            is_blocked: row.get::<_, i32>(15).unwrap_or(0) != 0,
        })
    })?;

    let mut users = Vec::new();
    for row in rows {
        users.push(row?);
    }
    Ok(users)
}

/// Structure for a task entry in the DB queue
#[derive(Debug, Clone)]
pub struct TaskQueueEntry {
    pub id: String,
    pub user_id: i64,
    pub url: String,
    pub format: String,
    pub is_video: bool,
    pub video_quality: Option<String>,
    pub audio_bitrate: Option<String>,
    pub priority: i32,
    pub status: String,
    pub error_message: Option<String>,
    pub retry_count: i32,
    pub created_at: String,
    pub updated_at: String,
}

/// Saves a task to the DB queue
#[allow(clippy::too_many_arguments)]
pub fn save_task_to_queue(
    conn: &DbConnection,
    task_id: &str,
    user_id: i64,
    url: &str,
    format: &str,
    is_video: bool,
    video_quality: Option<&str>,
    audio_bitrate: Option<&str>,
    priority: i32,
) -> Result<()> {
    conn.execute(
        "INSERT INTO task_queue (id, user_id, url, format, is_video, video_quality, audio_bitrate, priority, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending')
         ON CONFLICT(id) DO UPDATE SET
         status = 'pending',
         updated_at = CURRENT_TIMESTAMP,
         retry_count = 0,
         error_message = NULL",
        [
            &task_id as &dyn rusqlite::ToSql,
            &user_id as &dyn rusqlite::ToSql,
            &url as &dyn rusqlite::ToSql,
            &format as &dyn rusqlite::ToSql,
            &(if is_video { 1 } else { 0 }) as &dyn rusqlite::ToSql,
            &video_quality as &dyn rusqlite::ToSql,
            &audio_bitrate as &dyn rusqlite::ToSql,
            &priority as &dyn rusqlite::ToSql,
        ],
    )?;
    Ok(())
}

/// Updates the status of a task
pub fn update_task_status(conn: &DbConnection, task_id: &str, status: &str, error_message: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE task_queue SET status = ?1, error_message = ?2, updated_at = CURRENT_TIMESTAMP WHERE id = ?3",
        [
            &status as &dyn rusqlite::ToSql,
            &error_message as &dyn rusqlite::ToSql,
            &task_id as &dyn rusqlite::ToSql,
        ],
    )?;
    Ok(())
}

/// Increments the retry counter and updates the status to failed
pub fn mark_task_failed(conn: &DbConnection, task_id: &str, error_message: &str) -> Result<()> {
    conn.execute(
        "UPDATE task_queue
         SET status = 'failed',
             error_message = ?1,
             retry_count = retry_count + 1,
             updated_at = CURRENT_TIMESTAMP
         WHERE id = ?2",
        [&error_message as &dyn rusqlite::ToSql, &task_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Gets all failed tasks for reprocessing
pub fn get_failed_tasks(conn: &DbConnection, max_retries: i32) -> Result<Vec<TaskQueueEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, url, format, is_video, video_quality, audio_bitrate, priority, status, error_message, retry_count, created_at, updated_at
         FROM task_queue
         WHERE status = 'failed' AND retry_count < ?1
         ORDER BY priority DESC, created_at ASC"
    )?;
    let rows = stmt.query_map([&max_retries as &dyn rusqlite::ToSql], |row| {
        Ok(TaskQueueEntry {
            id: row.get(0)?,
            user_id: row.get(1)?,
            url: row.get(2)?,
            format: row.get(3)?,
            is_video: row.get::<_, i32>(4)? == 1,
            video_quality: row.get(5)?,
            audio_bitrate: row.get(6)?,
            priority: row.get(7)?,
            status: row.get(8)?,
            error_message: row.get(9)?,
            retry_count: row.get(10)?,
            created_at: row.get(11)?,
            updated_at: row.get(12)?,
        })
    })?;

    let mut tasks = Vec::new();
    for row in rows {
        tasks.push(row?);
    }
    Ok(tasks)
}

/// Gets a task by ID
pub fn get_task_by_id(conn: &DbConnection, task_id: &str) -> Result<Option<TaskQueueEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, url, format, is_video, video_quality, audio_bitrate, priority, status, error_message, retry_count, created_at, updated_at
         FROM task_queue WHERE id = ?1"
    )?;
    let mut rows = stmt.query_map([&task_id as &dyn rusqlite::ToSql], |row| {
        Ok(TaskQueueEntry {
            id: row.get(0)?,
            user_id: row.get(1)?,
            url: row.get(2)?,
            format: row.get(3)?,
            is_video: row.get::<_, i32>(4)? == 1,
            video_quality: row.get(5)?,
            audio_bitrate: row.get(6)?,
            priority: row.get(7)?,
            status: row.get(8)?,
            error_message: row.get(9)?,
            retry_count: row.get(10)?,
            created_at: row.get(11)?,
            updated_at: row.get(12)?,
        })
    })?;

    if let Some(row) = rows.next() {
        Ok(Some(row?))
    } else {
        Ok(None)
    }
}

/// Marks a task as completed
pub fn mark_task_completed(conn: &DbConnection, task_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE task_queue SET status = 'completed', updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
        [&task_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Marks a task as processing
pub fn mark_task_processing(conn: &DbConnection, task_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE task_queue SET status = 'processing', updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
        [&task_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Gets all recoverable tasks — pending or processing (interrupted by restart).
/// Resets their status to 'pending' atomically and returns them ordered by priority.
pub fn get_and_reset_recoverable_tasks(conn: &DbConnection) -> Result<Vec<TaskQueueEntry>> {
    // First reset processing → pending (these were interrupted mid-download)
    // Only consider tasks created within the last 24 hours
    conn.execute(
        "UPDATE task_queue SET status = 'pending', updated_at = CURRENT_TIMESTAMP
         WHERE status = 'processing'
         AND created_at > datetime('now', '-1 day')",
        [],
    )?;

    let mut stmt = conn.prepare(
        "SELECT id, user_id, url, format, is_video, video_quality, audio_bitrate, priority, status, error_message, retry_count, created_at, updated_at
         FROM task_queue
         WHERE status = 'pending'
         AND created_at > datetime('now', '-1 day')
         ORDER BY priority DESC, created_at ASC"
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(TaskQueueEntry {
            id: row.get(0)?,
            user_id: row.get(1)?,
            url: row.get(2)?,
            format: row.get(3)?,
            is_video: row.get::<_, i32>(4)? == 1,
            video_quality: row.get(5)?,
            audio_bitrate: row.get(6)?,
            priority: row.get(7)?,
            status: row.get(8)?,
            error_message: row.get(9)?,
            retry_count: row.get(10)?,
            created_at: row.get(11)?,
            updated_at: row.get(12)?,
        })
    })?;

    let mut tasks = Vec::new();
    for row in rows {
        tasks.push(row?);
    }
    Ok(tasks)
}

/// Updates the telegram_charge_id of a user (used for subscription management)
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `charge_id` - Telegram payment charge ID from a successful payment
///
/// # Returns
///
/// Returns `Ok(())` on success or a database error.
pub fn update_telegram_charge_id(conn: &DbConnection, telegram_id: i64, charge_id: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE users SET telegram_charge_id = ?1 WHERE telegram_id = ?2",
        [&charge_id as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

// ==================== Bot Assets ====================

/// Get a cached bot asset file_id by key (e.g. "ringtone_instruction_iphone_1")
pub fn get_bot_asset(conn: &DbConnection, key: &str) -> Result<Option<String>> {
    let result = conn.query_row("SELECT file_id FROM bot_assets WHERE key = ?1", [key], |row| row.get(0));
    match result {
        Ok(file_id) => Ok(Some(file_id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Set (upsert) a bot asset file_id for a key
pub fn set_bot_asset(conn: &DbConnection, key: &str, file_id: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO bot_assets (key, file_id, created_at) VALUES (?1, ?2, datetime('now'))",
        rusqlite::params![key, file_id],
    )?;
    Ok(())
}

// ==================== Video Timestamps ====================

use crate::timestamps::{TimestampSource, VideoTimestamp};

/// Save timestamps extracted from a video for later use in clip suggestions
pub fn save_video_timestamps(conn: &DbConnection, download_id: i64, timestamps: &[VideoTimestamp]) -> Result<()> {
    for ts in timestamps {
        conn.execute(
            "INSERT INTO video_timestamps (download_id, source, time_seconds, end_seconds, label)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                download_id,
                ts.source.as_str(),
                ts.time_seconds,
                ts.end_seconds,
                ts.label,
            ],
        )?;
    }
    Ok(())
}

/// Get timestamps for a download entry
pub fn get_video_timestamps(conn: &DbConnection, download_id: i64) -> Result<Vec<VideoTimestamp>> {
    let mut stmt = conn.prepare(
        "SELECT source, time_seconds, end_seconds, label
         FROM video_timestamps
         WHERE download_id = ?1
         ORDER BY time_seconds ASC",
    )?;

    let rows = stmt.query_map([download_id], |row| {
        let source_str: String = row.get(0)?;
        Ok(VideoTimestamp {
            source: TimestampSource::parse(&source_str),
            time_seconds: row.get(1)?,
            end_seconds: row.get(2)?,
            label: row.get(3)?,
        })
    })?;

    rows.collect()
}

/// Delete timestamps for a download entry
pub fn delete_video_timestamps(conn: &DbConnection, download_id: i64) -> Result<()> {
    conn.execute("DELETE FROM video_timestamps WHERE download_id = ?1", [download_id])?;
    Ok(())
}

// ==================== Cuts ====================

#[derive(Debug, Clone)]
pub struct CutEntry {
    pub id: i64,
    pub user_id: i64,
    pub original_url: String,
    pub source_kind: String,
    pub source_id: i64,
    pub output_kind: String,
    pub segments_json: String,
    pub segments_text: String,
    pub title: String,
    pub created_at: String,
    pub file_id: Option<String>,
    pub file_size: Option<i64>,
    pub duration: Option<i64>,
    pub video_quality: Option<String>,
}

pub fn create_cut(
    conn: &DbConnection,
    user_id: i64,
    original_url: &str,
    source_kind: &str,
    source_id: i64,
    output_kind: &str,
    segments_json: &str,
    segments_text: &str,
    title: &str,
    file_id: Option<&str>,
    file_size: Option<i64>,
    duration: Option<i64>,
    video_quality: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO cuts (
            user_id, original_url, source_kind, source_id, output_kind, segments_json, segments_text,
            title, file_id, file_size, duration, video_quality
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        rusqlite::params![
            user_id,
            original_url,
            source_kind,
            source_id,
            output_kind,
            segments_json,
            segments_text,
            title,
            file_id,
            file_size,
            duration,
            video_quality,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_cuts(conn: &DbConnection, user_id: i64, limit: Option<i32>) -> Result<Vec<CutEntry>> {
    let limit = limit.unwrap_or(50);
    let mut stmt = conn.prepare(
        "SELECT id, user_id, original_url, source_kind, source_id, output_kind, segments_json, segments_text,
                title, created_at, file_id, file_size, duration, video_quality
         FROM cuts
         WHERE user_id = ?1
         ORDER BY created_at DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![user_id, limit], |row| {
        Ok(CutEntry {
            id: row.get(0)?,
            user_id: row.get(1)?,
            original_url: row.get(2)?,
            source_kind: row.get(3)?,
            source_id: row.get(4)?,
            output_kind: row.get(5)?,
            segments_json: row.get(6)?,
            segments_text: row.get(7)?,
            title: row.get(8)?,
            created_at: row.get(9)?,
            file_id: row.get(10)?,
            file_size: row.get(11)?,
            duration: row.get(12)?,
            video_quality: row.get(13)?,
        })
    })?;

    rows.collect::<Result<Vec<_>>>()
}

pub fn get_cuts_count(conn: &DbConnection, user_id: i64) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM cuts WHERE user_id = ?1",
        rusqlite::params![user_id],
        |row| row.get(0),
    )
}

pub fn get_cuts_page(conn: &DbConnection, user_id: i64, limit: i64, offset: i64) -> Result<Vec<CutEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, original_url, source_kind, source_id, output_kind, segments_json, segments_text,
                title, created_at, file_id, file_size, duration, video_quality
         FROM cuts
         WHERE user_id = ?1
         ORDER BY created_at DESC
         LIMIT ?2 OFFSET ?3",
    )?;
    let rows = stmt.query_map(rusqlite::params![user_id, limit, offset], |row| {
        Ok(CutEntry {
            id: row.get(0)?,
            user_id: row.get(1)?,
            original_url: row.get(2)?,
            source_kind: row.get(3)?,
            source_id: row.get(4)?,
            output_kind: row.get(5)?,
            segments_json: row.get(6)?,
            segments_text: row.get(7)?,
            title: row.get(8)?,
            created_at: row.get(9)?,
            file_id: row.get(10)?,
            file_size: row.get(11)?,
            duration: row.get(12)?,
            video_quality: row.get(13)?,
        })
    })?;

    rows.collect::<Result<Vec<_>>>()
}

pub fn get_cut_entry(conn: &DbConnection, user_id: i64, cut_id: i64) -> Result<Option<CutEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, original_url, source_kind, source_id, output_kind, segments_json, segments_text,
                title, created_at, file_id, file_size, duration, video_quality
         FROM cuts
         WHERE id = ?1 AND user_id = ?2",
    )?;
    let mut rows = stmt.query(rusqlite::params![cut_id, user_id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(CutEntry {
            id: row.get(0)?,
            user_id: row.get(1)?,
            original_url: row.get(2)?,
            source_kind: row.get(3)?,
            source_id: row.get(4)?,
            output_kind: row.get(5)?,
            segments_json: row.get(6)?,
            segments_text: row.get(7)?,
            title: row.get(8)?,
            created_at: row.get(9)?,
            file_id: row.get(10)?,
            file_size: row.get(11)?,
            duration: row.get(12)?,
            video_quality: row.get(13)?,
        }))
    } else {
        Ok(None)
    }
}

// ==================== Subscription Management ====================

/// Gets the subscription record for a user from the subscriptions table.
pub fn get_subscription(conn: &DbConnection, telegram_id: i64) -> Result<Option<Subscription>> {
    let mut stmt = conn.prepare(
        "SELECT user_id, plan, expires_at, telegram_charge_id, is_recurring
         FROM subscriptions
         WHERE user_id = ?1",
    )?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(Some(Subscription {
            user_id: row.get(0)?,
            plan: row.get(1)?,
            expires_at: row.get::<_, Option<String>>(2)?,
            telegram_charge_id: row.get::<_, Option<String>>(3)?,
            is_recurring: row.get::<_, i32>(4).unwrap_or(0) != 0,
        }))
    } else {
        Ok(None)
    }
}

/// Updates the subscription data for a user after a successful payment.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `plan` - New user plan (e.g. "premium", "vip")
/// * `charge_id` - Telegram payment charge ID from a successful payment
/// * `subscription_expires_at` - Subscription expiry date (Unix timestamp or ISO 8601 string)
/// * `is_recurring` - Recurring subscription flag (auto-renewal)
///
/// # Returns
///
/// Returns `Ok(())` on success or a database error.
pub fn update_subscription_data(
    conn: &DbConnection,
    telegram_id: i64,
    plan: &str,
    charge_id: &str,
    subscription_expires_at: &str,
    is_recurring: bool,
) -> Result<()> {
    conn.execute(
        "INSERT INTO subscriptions (user_id, plan, expires_at, telegram_charge_id, is_recurring)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(user_id) DO UPDATE SET
            plan = excluded.plan,
            expires_at = excluded.expires_at,
            telegram_charge_id = excluded.telegram_charge_id,
            is_recurring = excluded.is_recurring,
            updated_at = CURRENT_TIMESTAMP",
        [
            &telegram_id as &dyn rusqlite::ToSql,
            &plan as &dyn rusqlite::ToSql,
            &subscription_expires_at as &dyn rusqlite::ToSql,
            &charge_id as &dyn rusqlite::ToSql,
            &(if is_recurring { 1 } else { 0 }) as &dyn rusqlite::ToSql,
        ],
    )?;
    conn.execute(
        "UPDATE users SET plan = ?1 WHERE telegram_id = ?2",
        [&plan as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Checks whether the subscription for a user is active.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
///
/// # Returns
///
/// Returns `Ok(true)` if the subscription is active, `Ok(false)` if not or expired.
pub fn is_subscription_active(conn: &DbConnection, telegram_id: i64) -> Result<bool> {
    let subscription = get_subscription(conn, telegram_id)?;

    let Some(subscription) = subscription else {
        return Ok(false);
    };

    if subscription.plan == Plan::Free {
        return Ok(false);
    }

    if let Some(expires_at) = subscription.expires_at {
        let mut stmt = conn.prepare("SELECT datetime('now', 'utc') < datetime(?1)")?;
        let is_active: bool = stmt.query_row([&expires_at], |row| row.get(0))?;
        Ok(is_active)
    } else {
        Ok(true)
    }
}

/// Cancels a user's subscription (clears the is_recurring flag).
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
///
/// # Returns
///
/// Returns `Ok(())` on success or a database error.
///
/// # Note
///
/// This function only removes the auto-renewal flag. The user retains
/// access until the subscription expiry date (subscription_expires_at).
pub fn cancel_subscription(conn: &DbConnection, telegram_id: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO subscriptions (user_id, plan, is_recurring)
         VALUES (?1, 'free', 0)
         ON CONFLICT(user_id) DO UPDATE SET
            is_recurring = 0,
            updated_at = CURRENT_TIMESTAMP",
        [&telegram_id as &dyn rusqlite::ToSql],
    )?;
    conn.execute(
        "UPDATE users SET plan = 'free' WHERE telegram_id = ?1",
        [&telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Gets the subscription status information for a user.
///
/// # Returns
///
/// Returns a tuple: (plan, expires_at, is_recurring, is_active)
pub type SubscriptionStatus = (Plan, Option<String>, bool, bool);

pub fn get_subscription_status(conn: &DbConnection, telegram_id: i64) -> Result<Option<SubscriptionStatus>> {
    let subscription = get_subscription(conn, telegram_id)?;

    if let Some(subscription) = subscription {
        let is_active = is_subscription_active(conn, telegram_id)?;
        Ok(Some((
            subscription.plan,
            subscription.expires_at,
            subscription.is_recurring,
            is_active,
        )))
    } else {
        Ok(None)
    }
}

/// Saves payment (charge) information to the database.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `user_id` - Telegram ID of the user
/// * `plan` - Subscription plan ("premium" or "vip")
/// * `telegram_charge_id` - Payment ID from Telegram
/// * `provider_charge_id` - Payment ID from provider (optional)
/// * `currency` - Payment currency (e.g. "XTR" for Stars)
/// * `total_amount` - Total payment amount
/// * `invoice_payload` - Invoice payload
/// * `is_recurring` - Recurring subscription flag
/// * `is_first_recurring` - Flag for first recurring payment
/// * `subscription_expiration_date` - Subscription expiry date
///
/// # Returns
///
/// Returns `Result<i64>` with the ID of the created record or an error.
pub fn save_charge(
    conn: &DbConnection,
    user_id: i64,
    plan: &str,
    telegram_charge_id: &str,
    provider_charge_id: Option<&str>,
    currency: &str,
    total_amount: i64,
    invoice_payload: &str,
    is_recurring: bool,
    is_first_recurring: bool,
    subscription_expiration_date: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO charges (
            user_id, plan, telegram_charge_id, provider_charge_id, currency,
            total_amount, invoice_payload, is_recurring, is_first_recurring,
            subscription_expiration_date
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            user_id,
            plan,
            telegram_charge_id,
            provider_charge_id,
            currency,
            total_amount,
            invoice_payload,
            is_recurring as i32,
            is_first_recurring as i32,
            subscription_expiration_date,
        ],
    )?;

    Ok(conn.last_insert_rowid())
}

/// Gets all charges for a specific user.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `user_id` - Telegram ID of the user
///
/// # Returns
///
/// Returns `Result<Vec<Charge>>` with a list of all user payments.
pub fn get_user_charges(conn: &DbConnection, user_id: i64) -> Result<Vec<Charge>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, plan, telegram_charge_id, provider_charge_id, currency,
                total_amount, invoice_payload, is_recurring, is_first_recurring,
                subscription_expiration_date, payment_date, created_at
         FROM charges
         WHERE user_id = ?1
         ORDER BY payment_date DESC",
    )?;

    let charges = stmt.query_map([user_id], |row| {
        Ok(Charge {
            id: row.get(0)?,
            user_id: row.get(1)?,
            plan: row.get(2)?,
            telegram_charge_id: row.get(3)?,
            provider_charge_id: row.get(4)?,
            currency: row.get(5)?,
            total_amount: row.get(6)?,
            invoice_payload: row.get(7)?,
            is_recurring: row.get::<_, i32>(8)? != 0,
            is_first_recurring: row.get::<_, i32>(9)? != 0,
            subscription_expiration_date: row.get(10)?,
            payment_date: row.get(11)?,
            created_at: row.get(12)?,
        })
    })?;

    charges.collect()
}

/// Gets all charges from the database with optional filtering and pagination.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `plan_filter` - Filter by plan (None = all plans)
/// * `limit` - Maximum number of records (None = all)
/// * `offset` - Offset for pagination
///
/// # Returns
///
/// Returns `Result<Vec<Charge>>` with a list of all payments.
pub fn get_all_charges(
    conn: &DbConnection,
    plan_filter: Option<&str>,
    limit: Option<i64>,
    offset: i64,
) -> Result<Vec<Charge>> {
    let query = if let Some(plan) = plan_filter {
        format!(
            "SELECT id, user_id, plan, telegram_charge_id, provider_charge_id, currency,
                    total_amount, invoice_payload, is_recurring, is_first_recurring,
                    subscription_expiration_date, payment_date, created_at
             FROM charges
             WHERE plan = '{}'
             ORDER BY payment_date DESC
             LIMIT {} OFFSET {}",
            plan,
            limit.unwrap_or(-1),
            offset
        )
    } else {
        format!(
            "SELECT id, user_id, plan, telegram_charge_id, provider_charge_id, currency,
                    total_amount, invoice_payload, is_recurring, is_first_recurring,
                    subscription_expiration_date, payment_date, created_at
             FROM charges
             ORDER BY payment_date DESC
             LIMIT {} OFFSET {}",
            limit.unwrap_or(-1),
            offset
        )
    };

    let mut stmt = conn.prepare(&query)?;

    let charges = stmt.query_map([], |row| {
        Ok(Charge {
            id: row.get(0)?,
            user_id: row.get(1)?,
            plan: row.get(2)?,
            telegram_charge_id: row.get(3)?,
            provider_charge_id: row.get(4)?,
            currency: row.get(5)?,
            total_amount: row.get(6)?,
            invoice_payload: row.get(7)?,
            is_recurring: row.get::<_, i32>(8)? != 0,
            is_first_recurring: row.get::<_, i32>(9)? != 0,
            subscription_expiration_date: row.get(10)?,
            payment_date: row.get(11)?,
            created_at: row.get(12)?,
        })
    })?;

    charges.collect()
}

/// Gets payment statistics.
///
/// # Arguments
///
/// * `conn` - Database connection
///
/// # Returns
///
/// Returns a tuple (total_charges, total_amount, premium_count, vip_count, recurring_count).
pub fn get_charges_stats(conn: &DbConnection) -> Result<(i64, i64, i64, i64, i64)> {
    let mut stmt = conn.prepare(
        "SELECT
            COUNT(*) as total_charges,
            SUM(total_amount) as total_amount,
            SUM(CASE WHEN plan = 'premium' THEN 1 ELSE 0 END) as premium_count,
            SUM(CASE WHEN plan = 'vip' THEN 1 ELSE 0 END) as vip_count,
            SUM(CASE WHEN is_recurring = 1 THEN 1 ELSE 0 END) as recurring_count
         FROM charges",
    )?;

    stmt.query_row([], |row| {
        Ok((
            row.get(0)?,
            row.get::<_, Option<i64>>(1)?.unwrap_or(0),
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        ))
    })
}

/// Saves user feedback to the database.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `user_id` - Telegram ID of the user
/// * `username` - Username of the user (optional)
/// * `first_name` - First name of the user
/// * `message` - Feedback text
///
/// # Returns
///
/// Returns `Result<i64>` with the ID of the created record or an error.
pub fn save_feedback(
    conn: &DbConnection,
    user_id: i64,
    username: Option<&str>,
    first_name: &str,
    message: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO feedback_messages (user_id, username, first_name, message, status)
         VALUES (?1, ?2, ?3, ?4, 'new')",
        rusqlite::params![user_id, username, first_name, message],
    )?;

    Ok(conn.last_insert_rowid())
}

/// Gets all feedback messages with optional status filtering.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `status_filter` - Filter by status ("new", "read", "replied", None = all)
/// * `limit` - Maximum number of records (None = all)
/// * `offset` - Offset for pagination
///
/// # Returns
///
/// Returns `Result<Vec<FeedbackMessage>>` with a list of feedback messages.
pub fn get_feedback_messages(
    conn: &DbConnection,
    status_filter: Option<&str>,
    limit: Option<i64>,
    offset: i64,
) -> Result<Vec<FeedbackMessage>> {
    let query = if let Some(status) = status_filter {
        format!(
            "SELECT id, user_id, username, first_name, message, status,
                    admin_reply, created_at, replied_at
             FROM feedback_messages
             WHERE status = '{}'
             ORDER BY created_at DESC
             LIMIT {} OFFSET {}",
            status,
            limit.unwrap_or(-1),
            offset
        )
    } else {
        format!(
            "SELECT id, user_id, username, first_name, message, status,
                    admin_reply, created_at, replied_at
             FROM feedback_messages
             ORDER BY created_at DESC
             LIMIT {} OFFSET {}",
            limit.unwrap_or(-1),
            offset
        )
    };

    let mut stmt = conn.prepare(&query)?;

    let messages = stmt.query_map([], |row| {
        Ok(FeedbackMessage {
            id: row.get(0)?,
            user_id: row.get(1)?,
            username: row.get(2)?,
            first_name: row.get(3)?,
            message: row.get(4)?,
            status: row.get(5)?,
            admin_reply: row.get(6)?,
            created_at: row.get(7)?,
            replied_at: row.get(8)?,
        })
    })?;

    messages.collect()
}

/// Gets feedback messages for a specific user.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `user_id` - Telegram ID of the user
///
/// # Returns
///
/// Returns `Result<Vec<FeedbackMessage>>` with a list of user feedback messages.
pub fn get_user_feedback(conn: &DbConnection, user_id: i64) -> Result<Vec<FeedbackMessage>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, username, first_name, message, status,
                admin_reply, created_at, replied_at
         FROM feedback_messages
         WHERE user_id = ?1
         ORDER BY created_at DESC",
    )?;

    let messages = stmt.query_map([user_id], |row| {
        Ok(FeedbackMessage {
            id: row.get(0)?,
            user_id: row.get(1)?,
            username: row.get(2)?,
            first_name: row.get(3)?,
            message: row.get(4)?,
            status: row.get(5)?,
            admin_reply: row.get(6)?,
            created_at: row.get(7)?,
            replied_at: row.get(8)?,
        })
    })?;

    messages.collect()
}

/// Updates the status of a feedback message.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `feedback_id` - Feedback message ID
/// * `status` - New status ("new", "read", "replied")
///
/// # Returns
///
/// Returns `Result<()>` or an error.
pub fn update_feedback_status(conn: &DbConnection, feedback_id: i64, status: &str) -> Result<()> {
    conn.execute(
        "UPDATE feedback_messages SET status = ?1 WHERE id = ?2",
        rusqlite::params![status, feedback_id],
    )?;
    Ok(())
}

/// Adds an admin reply to a feedback message.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `feedback_id` - Feedback message ID
/// * `reply` - Reply text
///
/// # Returns
///
/// Returns `Result<()>` or an error.
pub fn add_feedback_reply(conn: &DbConnection, feedback_id: i64, reply: &str) -> Result<()> {
    conn.execute(
        "UPDATE feedback_messages
         SET admin_reply = ?1, status = 'replied', replied_at = CURRENT_TIMESTAMP
         WHERE id = ?2",
        rusqlite::params![reply, feedback_id],
    )?;
    Ok(())
}

/// Gets feedback statistics.
///
/// # Arguments
///
/// * `conn` - Database connection
///
/// # Returns
///
/// Returns a tuple (total_feedback, new_count, read_count, replied_count).
pub fn get_feedback_stats(conn: &DbConnection) -> Result<(i64, i64, i64, i64)> {
    let mut stmt = conn.prepare(
        "SELECT
            COUNT(*) as total_feedback,
            SUM(CASE WHEN status = 'new' THEN 1 ELSE 0 END) as new_count,
            SUM(CASE WHEN status = 'read' THEN 1 ELSE 0 END) as read_count,
            SUM(CASE WHEN status = 'replied' THEN 1 ELSE 0 END) as replied_count
         FROM feedback_messages",
    )?;

    stmt.query_row([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
}
// ==================== Error Log ====================

/// Error log entry
#[derive(Debug, Clone)]
pub struct ErrorLogEntry {
    pub id: i64,
    pub timestamp: String,
    pub user_id: Option<i64>,
    pub username: Option<String>,
    pub error_type: String,
    pub error_message: String,
    pub url: Option<String>,
    pub context: Option<String>,
    pub resolved: bool,
}

/// Logs an error to the database
pub fn log_error(
    conn: &DbConnection,
    user_id: Option<i64>,
    username: Option<&str>,
    error_type: &str,
    error_message: &str,
    url: Option<&str>,
    context: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO error_log (user_id, username, error_type, error_message, url, context)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![user_id, username, error_type, error_message, url, context],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Marks an error as resolved
pub fn mark_error_resolved(conn: &DbConnection, error_id: i64) -> Result<()> {
    conn.execute("UPDATE error_log SET resolved = 1 WHERE id = ?1", [error_id])?;
    Ok(())
}

/// Gets recent errors (last N hours)
pub fn get_recent_errors(conn: &DbConnection, hours: i64, limit: i64) -> Result<Vec<ErrorLogEntry>> {
    let since = chrono::Utc::now() - chrono::Duration::hours(hours);
    let since_str = since.format("%Y-%m-%d %H:%M:%S").to_string();

    let mut stmt = conn.prepare(
        "SELECT id, timestamp, user_id, username, error_type, error_message, url, context, resolved
         FROM error_log
         WHERE timestamp >= ?1
         ORDER BY timestamp DESC
         LIMIT ?2",
    )?;

    let rows = stmt.query_map(rusqlite::params![since_str, limit], |row| {
        Ok(ErrorLogEntry {
            id: row.get(0)?,
            timestamp: row.get(1)?,
            user_id: row.get(2)?,
            username: row.get(3)?,
            error_type: row.get(4)?,
            error_message: row.get(5)?,
            url: row.get(6)?,
            context: row.get(7)?,
            resolved: row.get::<_, i32>(8)? != 0,
        })
    })?;

    let mut errors = Vec::new();
    for row in rows.flatten() {
        errors.push(row);
    }
    Ok(errors)
}

/// Gets error count by type for a period
pub fn get_error_stats(conn: &DbConnection, hours: i64) -> Result<Vec<(String, i64)>> {
    let since = chrono::Utc::now() - chrono::Duration::hours(hours);
    let since_str = since.format("%Y-%m-%d %H:%M:%S").to_string();

    let mut stmt = conn.prepare(
        "SELECT error_type, COUNT(*) as cnt
         FROM error_log
         WHERE timestamp >= ?1
         GROUP BY error_type
         ORDER BY cnt DESC",
    )?;

    let rows = stmt.query_map([&since_str], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;

    let mut stats = Vec::new();
    for row in rows.flatten() {
        stats.push(row);
    }
    Ok(stats)
}

/// Gets total error count for a period
pub fn get_error_count(conn: &DbConnection, hours: i64) -> Result<i64> {
    let since = chrono::Utc::now() - chrono::Duration::hours(hours);
    let since_str = since.format("%Y-%m-%d %H:%M:%S").to_string();

    let count = conn.query_row(
        "SELECT COUNT(*) FROM error_log WHERE timestamp >= ?1",
        [&since_str],
        |row| row.get(0),
    )?;
    Ok(count)
}

/// Cleans up old error logs (older than N days)
pub fn cleanup_old_errors(conn: &DbConnection, days: i64) -> Result<usize> {
    let cutoff = chrono::Utc::now() - chrono::Duration::days(days);
    let cutoff_str = cutoff.format("%Y-%m-%d %H:%M:%S").to_string();

    let deleted = conn.execute("DELETE FROM error_log WHERE timestamp < ?1", [&cutoff_str])?;
    Ok(deleted)
}

/// Removes completed and failed tasks older than `days` from task_queue.
/// Returns the number of rows deleted.
pub fn cleanup_old_tasks(conn: &DbConnection, days: i64) -> Result<usize> {
    let deleted = conn.execute(
        "DELETE FROM task_queue
         WHERE status IN ('completed', 'failed')
         AND updated_at < datetime('now', ?1)",
        [&format!("-{} days", days)],
    )?;
    Ok(deleted)
}

// ==================== Lyrics Sessions ====================

/// Store fetched lyrics (with parsed sections as JSON) for later retrieval by section.
pub fn create_lyrics_session(
    conn: &DbConnection,
    id: &str,
    user_id: i64,
    artist: &str,
    title: &str,
    sections_json: &str,
    has_structure: bool,
) -> Result<()> {
    let now = chrono::Utc::now();
    let expires_at = now + chrono::Duration::hours(24);
    conn.execute(
        "INSERT INTO lyrics_sessions (id, user_id, artist, title, sections_json, has_structure, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            id,
            user_id,
            artist,
            title,
            sections_json,
            has_structure as i32,
            now.to_rfc3339(),
            expires_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

/// Retrieve a lyrics session by ID. Returns (artist, title, sections_json, has_structure).
pub fn get_lyrics_session(conn: &DbConnection, id: &str) -> Result<Option<(String, String, String, bool)>> {
    let result = conn.query_row(
        "SELECT artist, title, sections_json, has_structure FROM lyrics_sessions WHERE id = ?1 AND expires_at > ?2",
        rusqlite::params![id, chrono::Utc::now().to_rfc3339()],
        |row| {
            let has_structure: i32 = row.get(3)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                has_structure != 0,
            ))
        },
    );
    match result {
        Ok(v) => Ok(Some(v)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Delete expired lyrics sessions.
pub fn delete_expired_lyrics_sessions(conn: &DbConnection) -> Result<usize> {
    let deleted = conn.execute(
        "DELETE FROM lyrics_sessions WHERE expires_at < ?1",
        [chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tempfile::NamedTempFile;

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Helper function to create a test database with schema
    fn setup_test_db() -> DbPool {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let db_path = std::env::temp_dir().join(format!("doradura_test_{}_{}.db", std::process::id(), counter));

        // Remove existing file if any
        let _ = std::fs::remove_file(&db_path);

        let db_path_str = db_path.to_string_lossy().to_string();

        // Explicitly open and run migrations (use test-specific function without outer transaction)
        {
            let mut conn = Connection::open(&db_path_str).expect("Failed to open database");
            crate::storage::migrations::run_migrations_for_test(&mut conn).expect("Failed to run migrations");
        }

        // Now create the pool
        let manager = r2d2_sqlite::SqliteConnectionManager::file(&db_path_str);
        r2d2::Pool::builder()
            .max_size(5)
            .build(manager)
            .expect("Failed to create test database pool")
    }

    /// Helper to create a test database with an in-memory connection
    #[allow(dead_code)]
    fn setup_memory_db() -> Connection {
        let mut conn = Connection::open(":memory:").unwrap();
        crate::storage::migrations::run_migrations_for_test(&mut conn).unwrap();
        conn
    }

    // ==================== User CRUD Tests ====================

    #[test]
    fn test_create_user() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        let result = create_user(&conn, 12345, Some("testuser".to_string()));
        assert!(result.is_ok());

        // Verify user was created
        let user = get_user(&conn, 12345).unwrap();
        assert!(user.is_some());
        let user = user.unwrap();
        assert_eq!(user.telegram_id, 12345);
        assert_eq!(user.username, Some("testuser".to_string()));
        assert_eq!(user.plan, Plan::Free);
        assert_eq!(user.download_format, "mp3");
        assert_eq!(user.language, "en");
    }

    #[test]
    fn test_create_user_with_language() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        let result = create_user_with_language(&conn, 12346, Some("ruuser".to_string()), "ru");
        assert!(result.is_ok());

        let user = get_user(&conn, 12346).unwrap().unwrap();
        assert_eq!(user.language, "ru");
    }

    #[test]
    fn test_create_user_without_username() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        let result = create_user(&conn, 12347, None);
        assert!(result.is_ok());

        let user = get_user(&conn, 12347).unwrap().unwrap();
        assert_eq!(user.username, None);
    }

    #[test]
    fn test_get_nonexistent_user() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        let user = get_user(&conn, 99999).unwrap();
        assert!(user.is_none());
    }

    #[test]
    fn test_user_struct_methods() {
        let user = User {
            telegram_id: 123,
            username: Some("test".to_string()),
            plan: crate::core::types::Plan::Premium,
            download_format: "mp4".to_string(),
            download_subtitles: 1,
            video_quality: "1080p".to_string(),
            audio_bitrate: "320k".to_string(),
            send_as_document: 0,
            send_audio_as_document: 1,
            subscription_expires_at: None,
            telegram_charge_id: None,
            language: "en".to_string(),
            is_recurring: false,
            burn_subtitles: 0,
            progress_bar_style: "classic".to_string(),
            is_blocked: false,
        };

        assert_eq!(user.telegram_id(), 123);
        assert_eq!(user.download_format(), "mp4");
    }

    // ==================== User Settings Tests ====================

    #[test]
    fn test_download_format_settings() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12350, None).unwrap();

        // Default format
        let format = get_user_download_format(&conn, 12350).unwrap();
        assert_eq!(format, "mp3");

        // Change format
        set_user_download_format(&conn, 12350, "mp4").unwrap();
        let format = get_user_download_format(&conn, 12350).unwrap();
        assert_eq!(format, "mp4");
    }

    #[test]
    fn test_download_format_nonexistent_user() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        // Should return default "mp3" for nonexistent user
        let format = get_user_download_format(&conn, 99999).unwrap();
        assert_eq!(format, "mp3");
    }

    #[test]
    fn test_subtitles_settings() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12351, None).unwrap();

        // Default is disabled
        let enabled = get_user_download_subtitles(&conn, 12351).unwrap();
        assert!(!enabled);

        // Enable subtitles
        set_user_download_subtitles(&conn, 12351, true).unwrap();
        let enabled = get_user_download_subtitles(&conn, 12351).unwrap();
        assert!(enabled);

        // Disable subtitles
        set_user_download_subtitles(&conn, 12351, false).unwrap();
        let enabled = get_user_download_subtitles(&conn, 12351).unwrap();
        assert!(!enabled);
    }

    #[test]
    fn test_burn_subtitles_settings() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12352, None).unwrap();

        // Default is disabled
        let enabled = get_user_burn_subtitles(&conn, 12352).unwrap();
        assert!(!enabled);

        // Enable burn subtitles
        set_user_burn_subtitles(&conn, 12352, true).unwrap();
        let enabled = get_user_burn_subtitles(&conn, 12352).unwrap();
        assert!(enabled);
    }

    #[test]
    fn test_video_quality_settings() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12353, None).unwrap();

        // Default quality
        let quality = get_user_video_quality(&conn, 12353).unwrap();
        assert_eq!(quality, "best");

        // Change quality
        set_user_video_quality(&conn, 12353, "720p").unwrap();
        let quality = get_user_video_quality(&conn, 12353).unwrap();
        assert_eq!(quality, "720p");
    }

    #[test]
    fn test_audio_bitrate_settings() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12354, None).unwrap();

        // Default bitrate
        let bitrate = get_user_audio_bitrate(&conn, 12354).unwrap();
        assert_eq!(bitrate, "320k");

        // Change bitrate
        set_user_audio_bitrate(&conn, 12354, "192k").unwrap();
        let bitrate = get_user_audio_bitrate(&conn, 12354).unwrap();
        assert_eq!(bitrate, "192k");
    }

    #[test]
    fn test_send_as_document_settings() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12355, None).unwrap();

        // Default is 0 (Media)
        let value = get_user_send_as_document(&conn, 12355).unwrap();
        assert_eq!(value, 0);

        // Change to Document
        set_user_send_as_document(&conn, 12355, 1).unwrap();
        let value = get_user_send_as_document(&conn, 12355).unwrap();
        assert_eq!(value, 1);
    }

    #[test]
    fn test_send_audio_as_document_settings() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12356, None).unwrap();

        // Default is 0 (Media)
        let value = get_user_send_audio_as_document(&conn, 12356).unwrap();
        assert_eq!(value, 0);

        // Change to Document
        set_user_send_audio_as_document(&conn, 12356, 1).unwrap();
        let value = get_user_send_audio_as_document(&conn, 12356).unwrap();
        assert_eq!(value, 1);
    }

    #[test]
    fn test_language_settings() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12357, None).unwrap();

        // Change language
        set_user_language(&conn, 12357, "fr").unwrap();
        let lang = get_user_language(&conn, 12357).unwrap();
        assert_eq!(lang, "fr");
    }

    // ==================== Plan/Subscription Tests ====================

    #[test]
    fn test_update_user_plan() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12360, None).unwrap();

        // Update to premium
        update_user_plan(&conn, 12360, "premium").unwrap();
        let user = get_user(&conn, 12360).unwrap().unwrap();
        assert_eq!(user.plan, Plan::Premium);
    }

    #[test]
    fn test_update_user_plan_to_free_clears_subscription_metadata() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12364, None).unwrap();
        update_subscription_data(&conn, 12364, "vip", "charge_123", "2026-12-31 00:00:00", true).unwrap();

        update_user_plan(&conn, 12364, "free").unwrap();

        let user = get_user(&conn, 12364).unwrap().unwrap();
        assert_eq!(user.plan, Plan::Free);
        assert_eq!(user.subscription_expires_at, None);
        assert_eq!(user.telegram_charge_id, None);
        assert!(!user.is_recurring);
    }

    #[test]
    fn test_update_user_plan_with_expiry() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12361, None).unwrap();

        // Update with 30-day expiry
        update_user_plan_with_expiry(&conn, 12361, "vip", Some(30)).unwrap();
        let user = get_user(&conn, 12361).unwrap().unwrap();
        assert_eq!(user.plan, Plan::Vip);
        assert!(user.subscription_expires_at.is_some());
    }

    #[test]
    fn test_update_user_plan_without_expiry() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12362, None).unwrap();

        // Update without expiry (free plan)
        update_user_plan_with_expiry(&conn, 12362, "free", None).unwrap();
        let user = get_user(&conn, 12362).unwrap().unwrap();
        assert_eq!(user.plan, Plan::Free);
    }

    #[test]
    fn test_set_user_blocked_roundtrip() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12365, Some("blocked_user".to_string())).unwrap();
        assert!(!is_user_blocked(&conn, 12365).unwrap());

        set_user_blocked(&conn, 12365, true).unwrap();
        assert!(is_user_blocked(&conn, 12365).unwrap());

        let user = get_user(&conn, 12365).unwrap().unwrap();
        assert!(user.is_blocked);

        set_user_blocked(&conn, 12365, false).unwrap();
        assert!(!is_user_blocked(&conn, 12365).unwrap());
    }

    #[test]
    fn test_search_users_includes_blocked_flag() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12366, Some("searchable_admin_target".to_string())).unwrap();
        set_user_blocked(&conn, 12366, true).unwrap();

        let users = search_users(&conn, "searchable_admin").unwrap();
        let user = users
            .into_iter()
            .find(|user| user.telegram_id == 12366)
            .expect("user should be returned by search");

        assert!(user.is_blocked);
    }

    #[test]
    fn test_is_premium_or_vip() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12363, None).unwrap();

        // Free user is not premium
        let result = is_premium_or_vip(&conn, 12363).unwrap();
        assert!(!result);

        // Update to premium
        update_user_plan(&conn, 12363, "premium").unwrap();
        let result = is_premium_or_vip(&conn, 12363).unwrap();
        assert!(result);

        // Update to vip
        update_user_plan(&conn, 12363, "vip").unwrap();
        let result = is_premium_or_vip(&conn, 12363).unwrap();
        assert!(result);
    }

    #[test]
    fn test_is_premium_or_vip_nonexistent_user() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        let result = is_premium_or_vip(&conn, 99999).unwrap();
        assert!(!result);
    }

    // ==================== Download History Tests ====================

    #[test]
    fn test_save_and_get_download_history() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12370, None).unwrap();

        // Save download
        let id = save_download_history(
            &conn,
            12370,
            "https://youtube.com/watch?v=test",
            "Test Song",
            "mp3",
            Some("file123"),
            Some("Test Artist"),
            Some(5000000),
            Some(180),
            None,
            Some("320k"),
            None,
            None,
        )
        .unwrap();

        assert!(id > 0);

        // Get history
        let history = get_download_history(&conn, 12370, Some(10)).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].title, "Test Song");
        assert_eq!(history[0].format, "mp3");
        assert_eq!(history[0].file_id, Some("file123".to_string()));
    }

    #[test]
    fn test_download_history_with_parts() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12371, None).unwrap();

        // Save main download
        let main_id = save_download_history(
            &conn,
            12371,
            "https://example.com/video",
            "Long Video",
            "mp4",
            Some("main_file"),
            None,
            Some(500000000),
            Some(3600),
            Some("1080p"),
            None,
            None,
            None,
        )
        .unwrap();

        // Save part
        let _part_id = save_download_history(
            &conn,
            12371,
            "https://example.com/video",
            "Long Video (Part 1)",
            "mp4",
            Some("part1_file"),
            None,
            Some(100000000),
            Some(720),
            Some("1080p"),
            None,
            Some(main_id),
            Some(1),
        )
        .unwrap();

        let history = get_download_history(&conn, 12371, Some(10)).unwrap();
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn test_get_download_history_entry() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12372, None).unwrap();

        let id = save_download_history(
            &conn,
            12372,
            "https://example.com",
            "Test",
            "mp3",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let entry = get_download_history_entry(&conn, 12372, id).unwrap();
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().title, "Test");

        // Wrong user
        let entry = get_download_history_entry(&conn, 99999, id).unwrap();
        assert!(entry.is_none());
    }

    #[test]
    fn test_delete_download_history_entry() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12373, None).unwrap();

        let id = save_download_history(
            &conn,
            12373,
            "https://example.com",
            "Test",
            "mp3",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // Delete entry
        let deleted = delete_download_history_entry(&conn, 12373, id).unwrap();
        assert!(deleted);

        // Try to delete again (should fail)
        let deleted = delete_download_history_entry(&conn, 12373, id).unwrap();
        assert!(!deleted);
    }

    #[test]
    fn test_get_all_download_history() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12374, None).unwrap();

        for i in 0..5 {
            save_download_history(
                &conn,
                12374,
                &format!("https://example.com/{}", i),
                &format!("Test {}", i),
                "mp3",
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();
        }

        let all = get_all_download_history(&conn, 12374).unwrap();
        assert_eq!(all.len(), 5);
    }

    #[test]
    fn test_get_download_history_filtered() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12375, None).unwrap();

        // mp3 with file_id
        save_download_history(
            &conn,
            12375,
            "https://example.com/1",
            "Song 1",
            "mp3",
            Some("file1"),
            Some("Artist A"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // mp4 with file_id
        save_download_history(
            &conn,
            12375,
            "https://example.com/2",
            "Video 1",
            "mp4",
            Some("file2"),
            None,
            None,
            None,
            Some("720p"),
            None,
            None,
            None,
        )
        .unwrap();

        // mp3 without file_id (should be excluded)
        save_download_history(
            &conn,
            12375,
            "https://example.com/3",
            "Song 2",
            "mp3",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // srt (should be excluded)
        save_download_history(
            &conn,
            12375,
            "https://example.com/4",
            "Subtitles",
            "srt",
            Some("file4"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // No filter - should get mp3 and mp4 with file_id
        let filtered = get_download_history_filtered(&conn, 12375, None, None, None).unwrap();
        assert_eq!(filtered.len(), 2);

        // Filter by mp3
        let filtered = get_download_history_filtered(&conn, 12375, Some("mp3"), None, None).unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].format, "mp3");

        // Search by title
        let filtered = get_download_history_filtered(&conn, 12375, None, Some("Song"), None).unwrap();
        assert_eq!(filtered.len(), 1);

        // Search by author
        let filtered = get_download_history_filtered(&conn, 12375, None, Some("Artist A"), None).unwrap();
        assert_eq!(filtered.len(), 1);
    }

    // ==================== Task Queue Tests ====================

    #[test]
    fn test_task_queue_operations() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12380, None).unwrap();

        // Save task
        save_task_to_queue(
            &conn,
            "task-001",
            12380,
            "https://example.com",
            "mp3",
            false,
            None,
            Some("320k"),
            0,
        )
        .unwrap();

        // Get task
        let task = get_task_by_id(&conn, "task-001").unwrap();
        assert!(task.is_some());
        let task = task.unwrap();
        assert_eq!(task.status, "pending");
        assert_eq!(task.url, "https://example.com");
        assert!(!task.is_video);

        // Mark processing
        mark_task_processing(&conn, "task-001").unwrap();
        let task = get_task_by_id(&conn, "task-001").unwrap().unwrap();
        assert_eq!(task.status, "processing");

        // Mark completed
        mark_task_completed(&conn, "task-001").unwrap();
        let task = get_task_by_id(&conn, "task-001").unwrap().unwrap();
        assert_eq!(task.status, "completed");
    }

    #[test]
    fn test_task_queue_failure() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12381, None).unwrap();

        save_task_to_queue(
            &conn,
            "task-002",
            12381,
            "https://example.com",
            "mp4",
            true,
            Some("720p"),
            None,
            1,
        )
        .unwrap();

        // Mark failed
        mark_task_failed(&conn, "task-002", "Download error").unwrap();
        let task = get_task_by_id(&conn, "task-002").unwrap().unwrap();
        assert_eq!(task.status, "failed");
        assert_eq!(task.error_message, Some("Download error".to_string()));
        assert_eq!(task.retry_count, 1);

        // Get failed tasks
        let failed = get_failed_tasks(&conn, 3).unwrap();
        assert_eq!(failed.len(), 1);
    }

    #[test]
    fn test_update_task_status() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12382, None).unwrap();

        save_task_to_queue(
            &conn,
            "task-003",
            12382,
            "https://example.com",
            "mp3",
            false,
            None,
            None,
            0,
        )
        .unwrap();

        update_task_status(&conn, "task-003", "custom_status", Some("Custom error")).unwrap();
        let task = get_task_by_id(&conn, "task-003").unwrap().unwrap();
        assert_eq!(task.status, "custom_status");
        assert_eq!(task.error_message, Some("Custom error".to_string()));
    }

    // ==================== User Statistics Tests ====================

    #[test]
    fn test_get_user_stats() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12390, None).unwrap();

        // Add some downloads
        for i in 0..3 {
            save_download_history(
                &conn,
                12390,
                &format!("https://example.com/{}", i),
                &format!("Artist {} - Song {}", i % 2, i),
                "mp3",
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();
        }

        let stats = get_user_stats(&conn, 12390).unwrap();
        assert_eq!(stats.total_downloads, 3);
        assert!(stats.total_size > 0);
    }

    #[test]
    fn test_get_global_stats() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12391, None).unwrap();
        create_user(&conn, 12392, None).unwrap();

        save_download_history(
            &conn,
            12391,
            "https://example.com/1",
            "Song 1",
            "mp3",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        save_download_history(
            &conn,
            12392,
            "https://example.com/2",
            "Song 1",
            "mp3",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let stats = get_global_stats(&conn).unwrap();
        assert_eq!(stats.total_users, 2);
        assert_eq!(stats.total_downloads, 2);
        assert!(!stats.top_tracks.is_empty());
    }

    // ==================== Subscription Tests ====================

    #[test]
    fn test_subscription_operations() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12400, None).unwrap();

        // Get default subscription
        let sub = get_subscription(&conn, 12400).unwrap();
        assert!(sub.is_some());
        let sub = sub.unwrap();
        assert_eq!(sub.plan, Plan::Free);

        // Update subscription
        update_subscription_data(&conn, 12400, "premium", "charge_123", "2099-12-31T23:59:59Z", true).unwrap();

        let sub = get_subscription(&conn, 12400).unwrap().unwrap();
        assert_eq!(sub.plan, Plan::Premium);
        assert_eq!(sub.telegram_charge_id, Some("charge_123".to_string()));
        assert!(sub.is_recurring);

        // Check if active
        let active = is_subscription_active(&conn, 12400).unwrap();
        assert!(active);
    }

    #[test]
    fn test_cancel_subscription() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12401, None).unwrap();

        update_subscription_data(&conn, 12401, "premium", "charge_456", "2099-12-31T23:59:59Z", true).unwrap();

        cancel_subscription(&conn, 12401).unwrap();

        // cancel_subscription disables auto-renewal
        let sub = get_subscription(&conn, 12401).unwrap().unwrap();
        assert!(!sub.is_recurring, "is_recurring should be false after cancel");

        // The subscription plan in subscriptions table remains unchanged
        // (only is_recurring is updated in ON CONFLICT clause)
        // get_user reads from COALESCE(s.plan, u.plan) - subscriptions table takes precedence
        // This is the actual behavior - user keeps premium until expiry
    }

    #[test]
    fn test_cancel_subscription_for_new_user() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        // User without existing subscription
        create_user(&conn, 12411, None).unwrap();

        cancel_subscription(&conn, 12411).unwrap();

        // For new users (INSERT path), plan is set to 'free'
        let sub = get_subscription(&conn, 12411).unwrap().unwrap();
        assert_eq!(sub.plan, Plan::Free);
        assert!(!sub.is_recurring);
    }

    #[test]
    fn test_get_subscription_status() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12402, None).unwrap();

        update_subscription_data(&conn, 12402, "vip", "charge_789", "2099-12-31T23:59:59Z", false).unwrap();

        let status = get_subscription_status(&conn, 12402).unwrap();
        assert!(status.is_some());
        let (plan, expires, recurring, active) = status.unwrap();
        assert_eq!(plan, Plan::Vip);
        assert!(expires.is_some());
        assert!(!recurring);
        assert!(active);
    }

    // ==================== Charge Tests ====================

    #[test]
    fn test_save_and_get_charges() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12410, None).unwrap();

        let id = save_charge(
            &conn,
            12410,
            "premium",
            "tg_charge_001",
            Some("provider_001"),
            "XTR",
            100,
            "premium_monthly",
            true,
            true,
            Some("2099-12-31T23:59:59Z"),
        )
        .unwrap();

        assert!(id > 0);

        let charges = get_user_charges(&conn, 12410).unwrap();
        assert_eq!(charges.len(), 1);
        assert_eq!(charges[0].plan, Plan::Premium);
        assert_eq!(charges[0].total_amount, 100);
        assert!(charges[0].is_recurring);
    }

    #[test]
    fn test_get_all_charges() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12411, None).unwrap();

        save_charge(
            &conn,
            12411,
            "premium",
            "tg_charge_002",
            None,
            "XTR",
            100,
            "premium",
            false,
            false,
            None,
        )
        .unwrap();

        save_charge(
            &conn,
            12411,
            "vip",
            "tg_charge_003",
            None,
            "XTR",
            200,
            "vip",
            false,
            false,
            None,
        )
        .unwrap();

        // Get all
        let all = get_all_charges(&conn, None, None, 0).unwrap();
        assert_eq!(all.len(), 2);

        // Filter by plan
        let premium_only = get_all_charges(&conn, Some("premium"), None, 0).unwrap();
        assert_eq!(premium_only.len(), 1);
    }

    #[test]
    fn test_get_charges_stats() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12412, None).unwrap();

        save_charge(&conn, 12412, "premium", "c1", None, "XTR", 100, "p", true, false, None).unwrap();
        save_charge(&conn, 12412, "vip", "c2", None, "XTR", 200, "v", false, false, None).unwrap();

        let (total, amount, premium, vip, recurring) = get_charges_stats(&conn).unwrap();
        assert_eq!(total, 2);
        assert_eq!(amount, 300);
        assert_eq!(premium, 1);
        assert_eq!(vip, 1);
        assert_eq!(recurring, 1);
    }

    // ==================== Feedback Tests ====================

    #[test]
    fn test_feedback_operations() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12420, Some("testuser".to_string())).unwrap();

        // Save feedback
        let id = save_feedback(&conn, 12420, Some("testuser"), "John", "Great bot!").unwrap();
        assert!(id > 0);

        // Get feedback
        let messages = get_feedback_messages(&conn, None, None, 0).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].message, "Great bot!");
        assert_eq!(messages[0].status, "new");

        // Update status
        update_feedback_status(&conn, id, "read").unwrap();
        let messages = get_feedback_messages(&conn, Some("read"), None, 0).unwrap();
        assert_eq!(messages.len(), 1);

        // Add reply
        add_feedback_reply(&conn, id, "Thank you!").unwrap();
        let messages = get_feedback_messages(&conn, Some("replied"), None, 0).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].admin_reply, Some("Thank you!".to_string()));
    }

    #[test]
    fn test_get_feedback_stats() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12421, None).unwrap();

        save_feedback(&conn, 12421, None, "User1", "Message 1").unwrap();
        let id2 = save_feedback(&conn, 12421, None, "User2", "Message 2").unwrap();
        update_feedback_status(&conn, id2, "read").unwrap();

        let (total, new, read, replied) = get_feedback_stats(&conn).unwrap();
        assert_eq!(total, 2);
        assert_eq!(new, 1);
        assert_eq!(read, 1);
        assert_eq!(replied, 0);
    }

    // ==================== Video Clip Session Tests ====================

    #[test]
    fn test_video_clip_session_operations() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12430, None).unwrap();

        let session = VideoClipSession {
            id: "vcs-001".to_string(),
            user_id: 12430,
            source_download_id: 1,
            source_kind: "download".to_string(),
            source_id: 1,
            original_url: "https://example.com".to_string(),
            output_kind: "cut".to_string(),
            created_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            subtitle_lang: None,
        };

        upsert_video_clip_session(&conn, &session).unwrap();

        let active = get_active_video_clip_session(&conn, 12430).unwrap();
        assert!(active.is_some());
        assert_eq!(active.unwrap().id, "vcs-001");

        delete_video_clip_session_by_user(&conn, 12430).unwrap();
        let active = get_active_video_clip_session(&conn, 12430).unwrap();
        assert!(active.is_none());
    }

    // ==================== Cut Tests ====================

    #[test]
    fn test_cut_operations() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12440, None).unwrap();

        let id = create_cut(
            &conn,
            12440,
            "https://example.com",
            "download",
            1,
            "cut",
            "[{\"start\": 0, \"end\": 10}]",
            "0:00 - 0:10",
            "My Cut",
            Some("file_cut_1"),
            Some(1000000),
            Some(10),
            Some("720p"),
        )
        .unwrap();

        assert!(id > 0);

        let cuts = get_cuts(&conn, 12440, Some(10)).unwrap();
        assert_eq!(cuts.len(), 1);
        assert_eq!(cuts[0].title, "My Cut");

        let entry = get_cut_entry(&conn, 12440, id).unwrap();
        assert!(entry.is_some());

        let count = get_cuts_count(&conn, 12440).unwrap();
        assert_eq!(count, 1);

        let page = get_cuts_page(&conn, 12440, 10, 0).unwrap();
        assert_eq!(page.len(), 1);
    }

    // ==================== Cookies Upload Session Tests ====================

    #[test]
    fn test_cookies_upload_session_operations() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12450, None).unwrap();

        let session = CookiesUploadSession {
            id: "cookie-001".to_string(),
            user_id: 12450,
            created_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
        };

        upsert_cookies_upload_session(&conn, &session).unwrap();

        let active = get_active_cookies_upload_session(&conn, 12450).unwrap();
        assert!(active.is_some());
        assert_eq!(active.unwrap().id, "cookie-001");

        delete_cookies_upload_session_by_user(&conn, 12450).unwrap();
        let active = get_active_cookies_upload_session(&conn, 12450).unwrap();
        assert!(active.is_none());
    }

    // ==================== Audio Cut Session Tests ====================

    #[test]
    fn test_audio_cut_session_operations() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12460, None).unwrap();

        let session = AudioCutSession {
            id: "acs-001".to_string(),
            user_id: 12460,
            audio_session_id: "audio-001".to_string(),
            created_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
        };

        upsert_audio_cut_session(&conn, &session).unwrap();

        let active = get_active_audio_cut_session(&conn, 12460).unwrap();
        assert!(active.is_some());
        assert_eq!(active.unwrap().id, "acs-001");

        delete_audio_cut_session_by_user(&conn, 12460).unwrap();
        let active = get_active_audio_cut_session(&conn, 12460).unwrap();
        assert!(active.is_none());
    }

    // ==================== Request History Tests ====================

    #[test]
    #[ignore = "request_history table not in migrations"]
    fn test_log_request() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12470, None).unwrap();

        let result = log_request(&conn, 12470, "https://youtube.com/watch?v=test");
        assert!(result.is_ok());
    }

    // ==================== Message ID Update Tests ====================

    #[test]
    fn test_update_download_message_id() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12480, None).unwrap();

        let id = save_download_history(
            &conn,
            12480,
            "https://example.com",
            "Test",
            "mp3",
            Some("file_id"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        update_download_message_id(&conn, id, 123, 12480).unwrap();

        let info = get_download_message_info(&conn, id).unwrap();
        assert!(info.is_some());
        let (msg_id, chat_id) = info.unwrap();
        assert_eq!(msg_id, 123);
        assert_eq!(chat_id, 12480);
    }

    // ==================== All Users Test ====================

    #[test]
    fn test_get_all_users() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12490, Some("user1".to_string())).unwrap();
        create_user(&conn, 12491, Some("user2".to_string())).unwrap();

        let users = get_all_users(&conn).unwrap();
        assert!(users.len() >= 2);
    }

    // ==================== Sent Files Test ====================

    #[test]
    fn test_get_sent_files() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12500, Some("sender".to_string())).unwrap();

        save_download_history(
            &conn,
            12500,
            "https://example.com",
            "Test File",
            "mp3",
            Some("sent_file_id"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let files = get_sent_files(&conn, Some(10)).unwrap();
        assert!(!files.is_empty());
        assert_eq!(files[0].file_id, "sent_file_id");
    }

    // ==================== Expire Subscriptions Test ====================

    #[test]
    fn test_expire_old_subscriptions() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12510, None).unwrap();

        // Set expired subscription
        conn.execute(
            "UPDATE subscriptions SET plan = 'premium', expires_at = datetime('now', '-1 day') WHERE user_id = 12510",
            [],
        )
        .unwrap();

        let count = expire_old_subscriptions(&conn).unwrap();
        assert_eq!(count, 1);

        let user = get_user(&conn, 12510).unwrap().unwrap();
        assert_eq!(user.plan, Plan::Free);
    }

    // ==================== Connection Pool Tests ====================

    #[test]
    fn test_create_pool_and_get_connection() {
        let temp_file = NamedTempFile::new().unwrap();
        let db_path = temp_file.path().to_str().unwrap();

        let pool = create_pool(db_path).unwrap();
        let conn = get_connection(&pool);
        assert!(conn.is_ok());
    }

    #[test]
    fn test_multiple_connections() {
        let pool = setup_test_db();

        let conn1 = get_connection(&pool);
        let conn2 = get_connection(&pool);

        assert!(conn1.is_ok());
        assert!(conn2.is_ok());
    }

    // ==================== Bot Assets Tests ====================

    #[test]
    fn test_get_bot_asset_nonexistent() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        let result = get_bot_asset(&conn, "nonexistent_key").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_set_and_get_bot_asset() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        set_bot_asset(&conn, "ringtone_instruction_iphone_1", "file_id_abc123").unwrap();

        let result = get_bot_asset(&conn, "ringtone_instruction_iphone_1").unwrap();
        assert_eq!(result, Some("file_id_abc123".to_string()));
    }

    #[test]
    fn test_set_bot_asset_upserts_existing_key() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        set_bot_asset(&conn, "key1", "first_value").unwrap();
        set_bot_asset(&conn, "key1", "updated_value").unwrap();

        let result = get_bot_asset(&conn, "key1").unwrap();
        assert_eq!(result, Some("updated_value".to_string()));
    }

    #[test]
    fn test_multiple_bot_assets_are_independent() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        for i in 1..=6 {
            let key = format!("ringtone_instruction_iphone_{}", i);
            let fid = format!("file_id_iphone_{}", i);
            set_bot_asset(&conn, &key, &fid).unwrap();
        }

        for i in 1..=6 {
            let key = format!("ringtone_instruction_iphone_{}", i);
            let expected = format!("file_id_iphone_{}", i);
            let result = get_bot_asset(&conn, &key).unwrap();
            assert_eq!(result, Some(expected), "Mismatch at step {}", i);
        }
    }

    #[test]
    fn test_iphone_and_android_assets_are_independent() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        set_bot_asset(&conn, "ringtone_instruction_iphone_1", "iphone_fid").unwrap();
        set_bot_asset(&conn, "ringtone_instruction_android_1", "android_fid").unwrap();

        assert_eq!(
            get_bot_asset(&conn, "ringtone_instruction_iphone_1").unwrap(),
            Some("iphone_fid".to_string())
        );
        assert_eq!(
            get_bot_asset(&conn, "ringtone_instruction_android_1").unwrap(),
            Some("android_fid".to_string())
        );
    }

    #[test]
    fn test_all_cached_detection_logic() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();
        let prefix = "ringtone_instruction_iphone_";
        let total = 3;

        // Before setting: none cached → not all_cached
        let cached: Vec<Option<String>> = (1..=total)
            .map(|i| get_bot_asset(&conn, &format!("{}{}", prefix, i)).ok().flatten())
            .collect();
        let all_cached = total > 0 && cached.iter().all(|id| id.is_some());
        assert!(!all_cached, "Should not be all_cached when nothing stored");

        // Set only 2 out of 3 — still not all_cached
        set_bot_asset(&conn, &format!("{}1", prefix), "fid_1").unwrap();
        set_bot_asset(&conn, &format!("{}2", prefix), "fid_2").unwrap();
        let cached: Vec<Option<String>> = (1..=total)
            .map(|i| get_bot_asset(&conn, &format!("{}{}", prefix, i)).ok().flatten())
            .collect();
        let all_cached = total > 0 && cached.iter().all(|id| id.is_some());
        assert!(!all_cached, "Should not be all_cached when only 2/3 set");

        // Set the last one — now all_cached
        set_bot_asset(&conn, &format!("{}3", prefix), "fid_3").unwrap();
        let cached: Vec<Option<String>> = (1..=total)
            .map(|i| get_bot_asset(&conn, &format!("{}{}", prefix, i)).ok().flatten())
            .collect();
        let all_cached = total > 0 && cached.iter().all(|id| id.is_some());
        assert!(all_cached, "Should be all_cached when all 3 set");
    }

    #[test]
    fn test_all_cached_zero_total_is_false() {
        // Edge case: if there are no images (total=0), all_cached must be false
        let total = 0usize;
        let cached: Vec<Option<String>> = vec![];
        let all_cached = total > 0 && cached.iter().all(|id| id.is_some());
        assert!(!all_cached, "all_cached with total=0 must be false");
    }
}
