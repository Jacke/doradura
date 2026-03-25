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
    // the entire migration batch (which would skip later migrations like V39).
    let _ = conn.execute_batch("ALTER TABLE users ADD COLUMN is_blocked INTEGER NOT NULL DEFAULT 0;");

    // V19: message_id/chat_id on download_history and cuts
    let _ = conn.execute_batch("ALTER TABLE download_history ADD COLUMN message_id INTEGER DEFAULT NULL;");
    let _ = conn.execute_batch("ALTER TABLE download_history ADD COLUMN chat_id INTEGER DEFAULT NULL;");
    let _ = conn.execute_batch("ALTER TABLE cuts ADD COLUMN message_id INTEGER DEFAULT NULL;");
    let _ = conn.execute_batch("ALTER TABLE cuts ADD COLUMN chat_id INTEGER DEFAULT NULL;");

    // V26: category on download_history
    let _ = conn.execute_batch("ALTER TABLE download_history ADD COLUMN category TEXT;");

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
    // V39: task_queue columns FIRST (SQLite has no ADD COLUMN IF NOT EXISTS)
    let alter_stmts = [
        "ALTER TABLE task_queue ADD COLUMN idempotency_key TEXT",
        "ALTER TABLE task_queue ADD COLUMN worker_id TEXT",
        "ALTER TABLE task_queue ADD COLUMN leased_at DATETIME",
        "ALTER TABLE task_queue ADD COLUMN lease_expires_at DATETIME",
        "ALTER TABLE task_queue ADD COLUMN last_heartbeat_at DATETIME",
        "ALTER TABLE task_queue ADD COLUMN execute_at DATETIME",
        "ALTER TABLE task_queue ADD COLUMN started_at DATETIME",
        "ALTER TABLE task_queue ADD COLUMN finished_at DATETIME",
        "ALTER TABLE task_queue ADD COLUMN message_id INTEGER",
        "ALTER TABLE task_queue ADD COLUMN time_range_start TEXT",
        "ALTER TABLE task_queue ADD COLUMN time_range_end TEXT",
        "ALTER TABLE task_queue ADD COLUMN carousel_mask INTEGER",
    ];
    for sql in &alter_stmts {
        let _ = conn.execute_batch(sql); // ignore "duplicate column" errors
    }

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
        // V39: processed_updates table
        "CREATE TABLE IF NOT EXISTS processed_updates (
            bot_id BIGINT NOT NULL,
            update_id BIGINT NOT NULL,
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (bot_id, update_id)
        )",
        "CREATE INDEX IF NOT EXISTS idx_processed_updates_created_at ON processed_updates(created_at)",
        // V39: task_queue indexes (columns already added above)
        "CREATE INDEX IF NOT EXISTS idx_task_queue_runnable ON task_queue(status, priority DESC, created_at ASC)",
        "CREATE INDEX IF NOT EXISTS idx_task_queue_lease_expiry ON task_queue(status, lease_expires_at)",
        "CREATE INDEX IF NOT EXISTS idx_task_queue_user_pending ON task_queue(user_id, status, created_at ASC)",
    ];
    for sql in &stmts {
        if let Err(e) = conn.execute_batch(sql) {
            log::warn!("ensure_tables: {}", e);
        }
    }

    // V39: unique index on idempotency_key (partial index)
    let _ = conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_task_queue_active_idempotency
            ON task_queue(idempotency_key)
            WHERE idempotency_key IS NOT NULL
              AND status IN ('pending', 'leased', 'processing', 'uploading')",
    );

    // preview_contexts: audio_lang column
    let _ = conn.execute_batch("ALTER TABLE preview_contexts ADD COLUMN audio_lang TEXT");

    // V41: player repeat/resume columns
    let _ = conn.execute_batch("ALTER TABLE player_sessions ADD COLUMN repeat_mode INTEGER NOT NULL DEFAULT 0");
    let _ = conn.execute_batch("ALTER TABLE player_sessions ADD COLUMN last_track_index INTEGER");

    // V42: experimental features
    let _ = conn.execute_batch("ALTER TABLE users ADD COLUMN experimental_features INTEGER DEFAULT 0");

    // V40: admin audit log
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS admin_audit_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            admin_id INTEGER NOT NULL,
            action TEXT NOT NULL,
            target_type TEXT NOT NULL,
            target_id TEXT NOT NULL,
            details TEXT,
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        )",
    );
    let _ = conn
        .execute_batch("CREATE INDEX IF NOT EXISTS idx_admin_audit_log_created ON admin_audit_log(created_at DESC)");
    let _ = conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_admin_audit_log_admin ON admin_audit_log(admin_id, created_at DESC)",
    );
    let _ = conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_admin_audit_log_action ON admin_audit_log(action)");
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
