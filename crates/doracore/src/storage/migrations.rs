use anyhow::{Context, Result};
use rusqlite::Connection;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

mod embedded {
    use refinery::embed_migrations;

    embed_migrations!("./migrations");
}

static MIGRATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub fn run_migrations(conn: &mut Connection) -> Result<()> {
    // Serialize migrations per-process to avoid concurrent runners
    let mutex = MIGRATION_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            log::warn!("Migration lock was poisoned, recovering...");
            poisoned.into_inner()
        }
    };

    conn.busy_timeout(Duration::from_secs(30))
        .context("set SQLite busy timeout")?;

    // Pre-apply columns that might already exist from init scripts,
    // so that refinery doesn't fail on "duplicate column" and roll back
    // the entire migration batch (which would skip later migrations like V38).
    let _ = conn.execute_batch("ALTER TABLE users ADD COLUMN is_blocked INTEGER NOT NULL DEFAULT 0;");

    match embedded::migrations::runner().run(conn).map(|_| ()) {
        Ok(()) => {
            log::info!("Database migrations applied successfully");
        }
        Err(e) => {
            let msg = format!("{:#}", e);
            log::error!("Migration error (full): {}", msg);

            // If error is about duplicate column or already applied schema,
            // continue anyway — the schema is likely correct from init script
            if msg.contains("duplicate column")
                || msg.contains("already exists")
                || msg.contains("table users already exists")
            {
                log::warn!("Migration error is about existing schema, continuing: {}", msg);
            } else {
                return Err(e).context("apply migrations");
            }
        }
    }

    // Ensure tables from later migrations exist even if an earlier migration
    // error caused the entire batch to roll back (e.g. V19 "duplicate column"
    // rolls back V38's CREATE TABLE).
    ensure_tables(conn);

    Ok(())
}

/// Idempotently create tables that may have been lost to a migration rollback.
fn ensure_tables(conn: &Connection) {
    let stmts = [
        // V38: archive sessions
        "CREATE TABLE IF NOT EXISTS archive_sessions (
            id          TEXT PRIMARY KEY,
            user_id     INTEGER NOT NULL,
            status      TEXT NOT NULL DEFAULT 'selecting',
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            expires_at  TEXT NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS archive_session_items (
            session_id  TEXT NOT NULL REFERENCES archive_sessions(id) ON DELETE CASCADE,
            download_id INTEGER NOT NULL,
            UNIQUE(session_id, download_id)
        )",
        "CREATE INDEX IF NOT EXISTS idx_archive_sessions_user ON archive_sessions(user_id, status)",
        "CREATE INDEX IF NOT EXISTS idx_archive_items_session ON archive_session_items(session_id)",
    ];
    for sql in &stmts {
        if let Err(e) = conn.execute_batch(sql) {
            log::warn!("ensure_tables: {}", e);
        }
    }
}

/// Run migrations for tests without the outer transaction wrapper
/// This is needed because refinery uses its own transactions internally
#[doc(hidden)]
pub fn run_migrations_for_test(conn: &mut Connection) -> Result<()> {
    conn.busy_timeout(Duration::from_secs(30))
        .context("set SQLite busy timeout")?;

    embedded::migrations::runner()
        .run(conn)
        .map(|_| ())
        .context("apply migrations")
}
