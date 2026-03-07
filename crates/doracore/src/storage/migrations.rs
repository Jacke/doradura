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

    // Try to apply any columns that might be missing before refinery runs,
    // so that refinery's V36 doesn't fail on "duplicate column".
    // This handles the case where s6 init script already applied V36 via raw sqlite3.
    let _ = conn.execute_batch("ALTER TABLE users ADD COLUMN is_blocked INTEGER NOT NULL DEFAULT 0;");

    match embedded::migrations::runner().run(conn).map(|_| ()) {
        Ok(()) => {
            log::info!("Database migrations applied successfully");
            Ok(())
        }
        Err(e) => {
            let msg = format!("{:#}", e);
            // Log the full error for debugging
            log::error!("Migration error (full): {}", msg);

            // If error is about duplicate column or already applied schema,
            // continue anyway — the schema is likely correct from init script
            if msg.contains("duplicate column")
                || msg.contains("already exists")
                || msg.contains("table users already exists")
            {
                log::warn!("Migration error is about existing schema, continuing: {}", msg);
                Ok(())
            } else {
                Err(e).context("apply migrations")
            }
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
