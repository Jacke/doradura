//! `SharedStorage` dispatch for the V50 `lyrics_overrides` table. SQLite branch
//! delegates to `storage/db/lyrics_overrides.rs`; Postgres is inline with the
//! same `ON CONFLICT DO UPDATE` semantics.

use anyhow::{Context, Result};
use sqlx::Row;

use crate::storage::db::{self, LyricsOverride};

use super::SharedStorage;

impl SharedStorage {
    /// Fetch the canonical lyrics override for a source key, if any.
    pub async fn get_lyrics_override(&self, source_key: &str) -> Result<Option<LyricsOverride>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_lyrics_override connection")?;
                db::get_lyrics_override(&conn, source_key).context("sqlite get_lyrics_override")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT source_key, provider, source_url, artist, title, lyrics_text
                     FROM lyrics_overrides WHERE source_key = $1",
                )
                .bind(source_key)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_lyrics_override")?;
                Ok(row.map(|r| LyricsOverride {
                    source_key: r.get("source_key"),
                    provider: r.get("provider"),
                    source_url: r.get("source_url"),
                    artist: r.get("artist"),
                    title: r.get("title"),
                    lyrics_text: r.get("lyrics_text"),
                }))
            }
        }
    }

    /// Insert or replace the override for a source key (last correction wins).
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_lyrics_override(
        &self,
        source_key: &str,
        provider: &str,
        source_url: &str,
        artist: Option<&str>,
        title: Option<&str>,
        lyrics_text: &str,
        corrected_by: Option<i64>,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite upsert_lyrics_override connection")?;
                db::upsert_lyrics_override(
                    &conn,
                    source_key,
                    provider,
                    source_url,
                    artist,
                    title,
                    lyrics_text,
                    corrected_by,
                )
                .context("sqlite upsert_lyrics_override")
            }
            Self::Postgres { pg_pool, .. } => {
                let now = chrono::Utc::now().to_rfc3339();
                sqlx::query(
                    "INSERT INTO lyrics_overrides
                        (source_key, provider, source_url, artist, title, lyrics_text, corrected_by, created_at, updated_at)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8)
                     ON CONFLICT(source_key) DO UPDATE SET
                        provider = excluded.provider,
                        source_url = excluded.source_url,
                        artist = excluded.artist,
                        title = excluded.title,
                        lyrics_text = excluded.lyrics_text,
                        corrected_by = excluded.corrected_by,
                        updated_at = excluded.updated_at",
                )
                .bind(source_key)
                .bind(provider)
                .bind(source_url)
                .bind(artist)
                .bind(title)
                .bind(lyrics_text)
                .bind(corrected_by)
                .bind(now)
                .execute(pg_pool)
                .await
                .context("postgres upsert_lyrics_override")?;
                Ok(())
            }
        }
    }
}
