//! SQLite operations on the V48 `popular_files` table.
//!
//! See migrations/V48__popular_files.sql for column commentary. The shared
//! `SharedStorage` wrapper lives at `storage/shared/popular_files.rs` and
//! dispatches to either this module or the Postgres branch.

use anyhow::Result;

use super::DbConnection;

/// One row of the `popular_files` cache. Lightweight by design — used only
/// to build inline-query results, not for full presentation.
#[derive(Debug, Clone, PartialEq)]
pub struct PopularFileEntry {
    pub url: String,
    pub format: String,
    pub file_id: String,
    pub title: Option<String>,
    pub author: Option<String>,
    pub duration: Option<i64>,
    pub file_size: Option<i64>,
    pub hits: i64,
}

/// Insert or bump a cache row. Returns the resulting hit count so callers
/// can log "first time this URL is popular!" events.
#[allow(clippy::too_many_arguments)]
pub fn upsert_popular_file(
    conn: &DbConnection,
    url: &str,
    format: &str,
    file_id: &str,
    title: Option<&str>,
    author: Option<&str>,
    duration: Option<i64>,
    file_size: Option<i64>,
) -> Result<i64> {
    // Race-safe upsert: on conflict we bump `hits` and `last_used` but keep
    // the original `first_seen` and overwrite `file_id` with the newest one
    // (older file_ids can rot in Telegram; freshest wins).
    conn.execute(
        "INSERT INTO popular_files (url, format, file_id, title, author, duration, file_size,
                                    hits, first_seen, last_used)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
         ON CONFLICT(url, format) DO UPDATE SET
             file_id    = excluded.file_id,
             title      = COALESCE(excluded.title, popular_files.title),
             author     = COALESCE(excluded.author, popular_files.author),
             duration   = COALESCE(excluded.duration, popular_files.duration),
             file_size  = COALESCE(excluded.file_size, popular_files.file_size),
             hits       = popular_files.hits + 1,
             last_used  = CURRENT_TIMESTAMP",
        rusqlite::params![url, format, file_id, title, author, duration, file_size],
    )?;
    let hits: i64 = conn.query_row(
        "SELECT hits FROM popular_files WHERE url = ?1 AND format = ?2",
        rusqlite::params![url, format],
        |row| row.get(0),
    )?;
    Ok(hits)
}

/// Look up a cached file for a (url, format) pair. `None` if not cached.
pub fn lookup_popular_file(conn: &DbConnection, url: &str, format: &str) -> Result<Option<PopularFileEntry>> {
    let mut stmt = conn.prepare(
        "SELECT url, format, file_id, title, author, duration, file_size, hits
         FROM popular_files
         WHERE url = ?1 AND format = ?2",
    )?;
    let row = stmt
        .query_map(rusqlite::params![url, format], |row| {
            Ok(PopularFileEntry {
                url: row.get(0)?,
                format: row.get(1)?,
                file_id: row.get(2)?,
                title: row.get(3)?,
                author: row.get(4)?,
                duration: row.get(5)?,
                file_size: row.get(6)?,
                hits: row.get(7)?,
            })
        })?
        .next()
        .transpose()?;
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::db::{create_pool, get_connection};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn setup_pool() -> crate::storage::db::DbPool {
        // create_pool() runs all embedded migrations including V48, so the
        // popular_files table exists out of the box — no manual CREATE here.
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("popular_files_test_{}_{}.db", std::process::id(), counter));
        let _ = fs_err::remove_file(&path);
        create_pool(path.to_string_lossy().as_ref()).unwrap()
    }

    #[test]
    fn upsert_then_lookup_returns_inserted_row() {
        let pool = setup_pool();
        let conn = get_connection(&pool).unwrap();
        let hits = upsert_popular_file(
            &conn,
            "https://youtu.be/abc",
            "mp3",
            "file_id_42",
            Some("Дора - Дорадура"),
            Some("Дора"),
            Some(201),
            Some(4_800_000),
        )
        .unwrap();
        assert_eq!(hits, 1);

        let found = lookup_popular_file(&conn, "https://youtu.be/abc", "mp3")
            .unwrap()
            .unwrap();
        assert_eq!(found.file_id, "file_id_42");
        assert_eq!(found.title.as_deref(), Some("Дора - Дорадура"));
        assert_eq!(found.hits, 1);
    }

    #[test]
    fn second_upsert_bumps_hits_and_replaces_file_id() {
        let pool = setup_pool();
        let conn = get_connection(&pool).unwrap();
        upsert_popular_file(&conn, "https://yt.be/x", "mp4", "old_id", None, None, None, None).unwrap();
        let hits = upsert_popular_file(&conn, "https://yt.be/x", "mp4", "new_id", None, None, None, None).unwrap();
        assert_eq!(hits, 2);

        let found = lookup_popular_file(&conn, "https://yt.be/x", "mp4").unwrap().unwrap();
        assert_eq!(found.file_id, "new_id");
        assert_eq!(found.hits, 2);
    }

    #[test]
    fn upsert_preserves_existing_title_when_new_is_none() {
        let pool = setup_pool();
        let conn = get_connection(&pool).unwrap();
        upsert_popular_file(
            &conn,
            "https://yt.be/y",
            "mp3",
            "id1",
            Some("Original title"),
            Some("Original artist"),
            None,
            None,
        )
        .unwrap();
        upsert_popular_file(&conn, "https://yt.be/y", "mp3", "id2", None, None, None, None).unwrap();

        let found = lookup_popular_file(&conn, "https://yt.be/y", "mp3").unwrap().unwrap();
        assert_eq!(found.title.as_deref(), Some("Original title"));
        assert_eq!(found.author.as_deref(), Some("Original artist"));
    }

    #[test]
    fn lookup_missing_returns_none() {
        let pool = setup_pool();
        let conn = get_connection(&pool).unwrap();
        assert!(
            lookup_popular_file(&conn, "https://nope.example/x", "mp3")
                .unwrap()
                .is_none()
        );
    }
}
