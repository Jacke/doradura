//! SQLite operations on the V50 `lyrics_overrides` table.
//!
//! Global (canonical) source→lyrics corrections: a [`LyricsOverride`] is keyed
//! by the canonicalized source URL and stores a snapshot of the corrected
//! lyrics. The shared wrapper lives at `storage/shared/lyrics_overrides.rs`.

use anyhow::Result;
use rusqlite::OptionalExtension;

use super::DbConnection;

/// One stored lyrics correction.
#[derive(Debug, Clone, PartialEq)]
pub struct LyricsOverride {
    pub source_key: String,
    pub provider: String,
    pub source_url: String,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub lyrics_text: String,
}

/// Fetch the override for a canonical source key, if any.
pub fn get_lyrics_override(conn: &DbConnection, source_key: &str) -> Result<Option<LyricsOverride>> {
    let row = conn
        .query_row(
            "SELECT source_key, provider, source_url, artist, title, lyrics_text
             FROM lyrics_overrides WHERE source_key = ?1",
            rusqlite::params![source_key],
            |r| {
                Ok(LyricsOverride {
                    source_key: r.get(0)?,
                    provider: r.get(1)?,
                    source_url: r.get(2)?,
                    artist: r.get(3)?,
                    title: r.get(4)?,
                    lyrics_text: r.get(5)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

/// Insert or replace the override for a source key (last correction wins).
#[allow(clippy::too_many_arguments)]
pub fn upsert_lyrics_override(
    conn: &DbConnection,
    source_key: &str,
    provider: &str,
    source_url: &str,
    artist: Option<&str>,
    title: Option<&str>,
    lyrics_text: &str,
    corrected_by: Option<i64>,
) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO lyrics_overrides
            (source_key, provider, source_url, artist, title, lyrics_text, corrected_by, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
         ON CONFLICT(source_key) DO UPDATE SET
            provider = excluded.provider,
            source_url = excluded.source_url,
            artist = excluded.artist,
            title = excluded.title,
            lyrics_text = excluded.lyrics_text,
            corrected_by = excluded.corrected_by,
            updated_at = excluded.updated_at",
        rusqlite::params![
            source_key,
            provider,
            source_url,
            artist,
            title,
            lyrics_text,
            corrected_by,
            now
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::db::{create_pool, get_connection};
    use std::sync::atomic::{AtomicU64, Ordering};

    static C: AtomicU64 = AtomicU64::new(0);

    fn pool() -> crate::storage::db::DbPool {
        let n = C.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("lyr_ovr_{}_{}.db", std::process::id(), n));
        let _ = fs_err::remove_file(&p);
        create_pool(p.to_string_lossy().as_ref()).unwrap()
    }

    #[test]
    fn upsert_then_get() {
        let pool = pool();
        let conn = get_connection(&pool).unwrap();
        assert!(get_lyrics_override(&conn, "k1").unwrap().is_none());

        upsert_lyrics_override(
            &conn,
            "k1",
            "genius",
            "https://genius.com/x",
            Some("Дора"),
            Some("Дорадура"),
            "la la",
            Some(42),
        )
        .unwrap();
        let got = get_lyrics_override(&conn, "k1").unwrap().unwrap();
        assert_eq!(got.provider, "genius");
        assert_eq!(got.artist.as_deref(), Some("Дора"));
        assert_eq!(got.lyrics_text, "la la");

        // Re-correct → last wins.
        upsert_lyrics_override(
            &conn,
            "k1",
            "lrclib",
            "https://lrclib.net/api/get/9",
            None,
            Some("T"),
            "new text",
            None,
        )
        .unwrap();
        let got = get_lyrics_override(&conn, "k1").unwrap().unwrap();
        assert_eq!(got.provider, "lrclib");
        assert_eq!(got.lyrics_text, "new text");
        assert_eq!(got.artist, None);
    }
}
