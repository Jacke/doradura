//! Database connection pool creation and management.

use super::DbPool;
use crate::storage::migrations;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Connection, Result};
use std::time::Duration;

use super::{CONNECTION_TIMEOUT_SECS, DbConnection};

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

    // `busy_timeout = 30000` (30s): tells SQLite to block and retry on
    // SQLITE_BUSY for up to 30 seconds before giving up. Default 5s was too
    // tight during active large-video downloads (1080p, 2-4 min pipeline)
    // where download-progress / metadata / history-insert / queue-lease /
    // log_request writers queue up and step over each other's reserved locks.
    // Symptom in production (2026-04-20): `Failed to claim next queue task:
    // sqlite claim_next_task` firing every 5s while a long download held the
    // writer slot, jamming the whole queue until the download finished.
    // 30s gives honest slack under contention without masking permanent locks.
    let manager = SqliteConnectionManager::file(database_path)
        .with_init(|conn| conn.execute_batch("PRAGMA busy_timeout = 30000;"));
    let pool = Pool::builder()
        .max_size(20) // Maximum 20 connections in the pool
        .connection_timeout(Duration::from_secs(CONNECTION_TIMEOUT_SECS)) // Prevent indefinite blocking
        .build(manager)?;

    // Enable WAL mode for better concurrent read performance (~5x throughput).
    // WAL is a persistent database-level setting — only needs to be set once,
    // but re-setting is a no-op so it's safe to do on every startup.
    match pool.get() {
        Ok(conn) => match conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;") {
            Err(e) => {
                log::warn!("Failed to enable WAL mode: {}", e);
            }
            _ => {
                log::info!("SQLite WAL mode enabled");
            }
        },
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
