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

    match embedded::migrations::runner().run(conn).map(|_| ()) {
        Ok(()) => Ok(()),
        Err(e) => {
            let msg = e.to_string();
            // If the only error is a duplicate column from V36 being applied
            // by the s6 init script, treat it as success
            if msg.contains("duplicate column name") {
                log::warn!(
                    "Migration had duplicate column error (likely from init script), ignoring: {}",
                    msg
                );
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
