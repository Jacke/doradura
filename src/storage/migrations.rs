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
    // Serialize migrations per-process and take an exclusive SQLite lock
    // to avoid concurrent runners interleaving on multi-instance startups.
    let _guard = MIGRATION_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("migration lock poisoned");

    conn.busy_timeout(Duration::from_secs(30))
        .context("set SQLite busy timeout")?;
    conn.execute_batch("BEGIN IMMEDIATE")
        .context("acquire migration lock")?;

    if let Err(err) = embedded::migrations::runner().run(conn).map(|_| ()) {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(err).context("apply migrations");
    }

    conn.execute_batch("COMMIT").context("commit migrations")?;
    Ok(())
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
