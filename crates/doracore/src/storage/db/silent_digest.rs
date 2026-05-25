//! SQLite operations on the V49 `silent_digest` table.
//!
//! See migrations/V49__silent_downloads.sql for column commentary. The shared
//! `SharedStorage` wrapper lives at `storage/shared/silent_digest.rs` and
//! dispatches to either this module or the Postgres branch.

use anyhow::Result;

use super::DbConnection;

/// One pending recap row: a silent download that finished (or failed) and is
/// waiting for the next-interaction MOTD. Lightweight — only what the digest
/// message needs.
#[derive(Debug, Clone, PartialEq)]
pub struct SilentDigestEntry {
    pub title: Option<String>,
    pub format: Option<String>,
    /// `"done"` or `"failed"`.
    pub status: String,
}

/// Record a completed (or failed) silent download awaiting a MOTD recap.
pub fn insert_silent_digest(
    conn: &DbConnection,
    user_id: i64,
    title: Option<&str>,
    format: Option<&str>,
    status: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO silent_digest (user_id, title, format, status) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![user_id, title, format, status],
    )?;
    Ok(())
}

/// Atomically fetch all unshown rows for a user and mark them shown, in a
/// single `UPDATE … RETURNING`. Idempotent: a concurrent second call (two
/// rapid interactions) returns an empty Vec, so the MOTD is never shown twice.
pub fn take_unshown_silent_digest(conn: &DbConnection, user_id: i64) -> Result<Vec<SilentDigestEntry>> {
    let mut stmt = conn.prepare(
        "UPDATE silent_digest SET shown = 1
         WHERE user_id = ?1 AND shown = 0
         RETURNING title, format, status",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![user_id], |row| {
            Ok(SilentDigestEntry {
                title: row.get(0)?,
                format: row.get(1)?,
                status: row.get(2)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::db::{create_pool, get_connection};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn setup_pool() -> crate::storage::db::DbPool {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("silent_digest_test_{}_{}.db", std::process::id(), counter));
        let _ = fs_err::remove_file(&path);
        create_pool(path.to_string_lossy().as_ref()).unwrap()
    }

    #[test]
    fn insert_then_take_returns_rows_once() {
        let pool = setup_pool();
        let conn = get_connection(&pool).unwrap();
        insert_silent_digest(&conn, 7, Some("Дора - Дорадура"), Some("mp3"), "done").unwrap();
        insert_silent_digest(&conn, 7, Some("Clip"), Some("mp4"), "failed").unwrap();

        let first = take_unshown_silent_digest(&conn, 7).unwrap();
        assert_eq!(first.len(), 2);
        assert!(
            first
                .iter()
                .any(|e| e.title.as_deref() == Some("Дора - Дорадура") && e.status == "done")
        );
        assert!(first.iter().any(|e| e.status == "failed"));

        // Idempotent: second call returns nothing.
        let second = take_unshown_silent_digest(&conn, 7).unwrap();
        assert!(second.is_empty());
    }

    #[test]
    fn take_is_scoped_per_user() {
        let pool = setup_pool();
        let conn = get_connection(&pool).unwrap();
        insert_silent_digest(&conn, 1, Some("A"), Some("mp3"), "done").unwrap();
        insert_silent_digest(&conn, 2, Some("B"), Some("mp3"), "done").unwrap();

        let u1 = take_unshown_silent_digest(&conn, 1).unwrap();
        assert_eq!(u1.len(), 1);
        assert_eq!(u1[0].title.as_deref(), Some("A"));

        // User 2's row is untouched by user 1's take.
        let u2 = take_unshown_silent_digest(&conn, 2).unwrap();
        assert_eq!(u2.len(), 1);
        assert_eq!(u2[0].title.as_deref(), Some("B"));
    }

    #[test]
    fn take_empty_for_unknown_user() {
        let pool = setup_pool();
        let conn = get_connection(&pool).unwrap();
        assert!(take_unshown_silent_digest(&conn, 999).unwrap().is_empty());
    }
}
