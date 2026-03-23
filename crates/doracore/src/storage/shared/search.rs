use anyhow::{Context, Result};
use rusqlite::OptionalExtension;
use sqlx::Row;

use crate::storage::db;

use super::types::PreviewContext;
use super::SharedStorage;

impl SharedStorage {
    pub async fn upsert_search_session(
        &self,
        user_id: i64,
        query: &str,
        results_json: &str,
        source: &str,
        context_kind: &str,
        playlist_id: Option<i64>,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite upsert_search_session connection")?;
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS search_sessions (
                        user_id INTEGER PRIMARY KEY,
                        query TEXT NOT NULL,
                        results_json TEXT NOT NULL,
                        source TEXT NOT NULL,
                        context_kind TEXT NOT NULL,
                        playlist_id INTEGER,
                        created_at TEXT NOT NULL DEFAULT (datetime('now'))
                    );
                    CREATE INDEX IF NOT EXISTS idx_search_sessions_created_at ON search_sessions(created_at);",
                )
                .context("sqlite ensure search_sessions table")?;
                conn.execute(
                    "INSERT OR REPLACE INTO search_sessions (
                        user_id, query, results_json, source, context_kind, playlist_id, created_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))",
                    rusqlite::params![user_id, query, results_json, source, context_kind, playlist_id],
                )
                .context("sqlite upsert_search_session")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO search_sessions (
                        user_id, query, results_json, source, context_kind, playlist_id, created_at
                     ) VALUES ($1, $2, $3, $4, $5, $6, NOW())
                     ON CONFLICT (user_id) DO UPDATE SET
                        query = EXCLUDED.query,
                        results_json = EXCLUDED.results_json,
                        source = EXCLUDED.source,
                        context_kind = EXCLUDED.context_kind,
                        playlist_id = EXCLUDED.playlist_id,
                        created_at = NOW()",
                )
                .bind(user_id)
                .bind(query)
                .bind(results_json)
                .bind(source)
                .bind(context_kind)
                .bind(playlist_id)
                .execute(pg_pool)
                .await
                .context("postgres upsert_search_session")?;
                Ok(())
            }
        }
    }

    pub async fn get_search_session(
        &self,
        user_id: i64,
        ttl_secs: i64,
    ) -> Result<Option<(String, String, String, String, Option<i64>)>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_search_session connection")?;
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS search_sessions (
                        user_id INTEGER PRIMARY KEY,
                        query TEXT NOT NULL,
                        results_json TEXT NOT NULL,
                        source TEXT NOT NULL,
                        context_kind TEXT NOT NULL,
                        playlist_id INTEGER,
                        created_at TEXT NOT NULL DEFAULT (datetime('now'))
                    );
                    CREATE INDEX IF NOT EXISTS idx_search_sessions_created_at ON search_sessions(created_at);",
                )
                .context("sqlite ensure search_sessions table")?;
                let row = conn
                    .query_row(
                        "SELECT query, results_json, source, context_kind, playlist_id
                         FROM search_sessions
                         WHERE user_id = ?1
                           AND datetime(created_at, '+' || ?2 || ' seconds') > datetime('now')",
                        rusqlite::params![user_id, ttl_secs],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
                    )
                    .optional()
                    .context("sqlite get_search_session")?;
                Ok(row)
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT query, results_json, source, context_kind, playlist_id
                     FROM search_sessions
                     WHERE user_id = $1
                       AND created_at > NOW() - ($2 * INTERVAL '1 second')",
                )
                .bind(user_id)
                .bind(ttl_secs)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_search_session")?;
                Ok(row.map(|row| {
                    (
                        row.get("query"),
                        row.get("results_json"),
                        row.get("source"),
                        row.get("context_kind"),
                        row.get("playlist_id"),
                    )
                }))
            }
        }
    }

    pub async fn delete_search_session(&self, user_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite delete_search_session connection")?;
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS search_sessions (
                        user_id INTEGER PRIMARY KEY,
                        query TEXT NOT NULL,
                        results_json TEXT NOT NULL,
                        source TEXT NOT NULL,
                        context_kind TEXT NOT NULL,
                        playlist_id INTEGER,
                        created_at TEXT NOT NULL DEFAULT (datetime('now'))
                    );
                    CREATE INDEX IF NOT EXISTS idx_search_sessions_created_at ON search_sessions(created_at);",
                )
                .context("sqlite ensure search_sessions table")?;
                conn.execute("DELETE FROM search_sessions WHERE user_id = ?1", [user_id])
                    .context("sqlite delete_search_session")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("DELETE FROM search_sessions WHERE user_id = $1")
                    .bind(user_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres delete_search_session")?;
                Ok(())
            }
        }
    }

    pub async fn upsert_prompt_session(
        &self,
        user_id: i64,
        kind: &str,
        payload_json: &str,
        ttl_secs: i64,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite upsert_prompt_session connection")?;
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS prompt_sessions (
                        user_id INTEGER NOT NULL,
                        kind TEXT NOT NULL,
                        payload_json TEXT NOT NULL DEFAULT '',
                        created_at TEXT NOT NULL DEFAULT (datetime('now')),
                        expires_at TEXT NOT NULL,
                        PRIMARY KEY (user_id, kind)
                    );
                    CREATE INDEX IF NOT EXISTS idx_prompt_sessions_expires_at ON prompt_sessions(expires_at);",
                )
                .context("sqlite ensure prompt_sessions table")?;
                conn.execute(
                    "INSERT OR REPLACE INTO prompt_sessions (
                        user_id, kind, payload_json, created_at, expires_at
                     ) VALUES (?1, ?2, ?3, datetime('now'), datetime('now', '+' || ?4 || ' seconds'))",
                    rusqlite::params![user_id, kind, payload_json, ttl_secs],
                )
                .context("sqlite upsert_prompt_session")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO prompt_sessions (
                        user_id, kind, payload_json, created_at, expires_at
                     ) VALUES ($1, $2, $3, NOW(), NOW() + ($4 * INTERVAL '1 second'))
                     ON CONFLICT (user_id, kind) DO UPDATE SET
                        payload_json = EXCLUDED.payload_json,
                        created_at = NOW(),
                        expires_at = EXCLUDED.expires_at",
                )
                .bind(user_id)
                .bind(kind)
                .bind(payload_json)
                .bind(ttl_secs)
                .execute(pg_pool)
                .await
                .context("postgres upsert_prompt_session")?;
                Ok(())
            }
        }
    }

    pub async fn get_prompt_session(&self, user_id: i64, kind: &str) -> Result<Option<String>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_prompt_session connection")?;
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS prompt_sessions (
                        user_id INTEGER NOT NULL,
                        kind TEXT NOT NULL,
                        payload_json TEXT NOT NULL DEFAULT '',
                        created_at TEXT NOT NULL DEFAULT (datetime('now')),
                        expires_at TEXT NOT NULL,
                        PRIMARY KEY (user_id, kind)
                    );
                    CREATE INDEX IF NOT EXISTS idx_prompt_sessions_expires_at ON prompt_sessions(expires_at);",
                )
                .context("sqlite ensure prompt_sessions table")?;
                let row = conn
                    .query_row(
                        "SELECT payload_json
                         FROM prompt_sessions
                         WHERE user_id = ?1
                           AND kind = ?2
                           AND expires_at > datetime('now')",
                        rusqlite::params![user_id, kind],
                        |row| row.get(0),
                    )
                    .optional()
                    .context("sqlite get_prompt_session")?;
                Ok(row)
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT payload_json
                     FROM prompt_sessions
                     WHERE user_id = $1
                       AND kind = $2
                       AND expires_at > NOW()",
                )
                .bind(user_id)
                .bind(kind)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_prompt_session")?;
                Ok(row.map(|row| row.get("payload_json")))
            }
        }
    }

    pub async fn delete_prompt_session(&self, user_id: i64, kind: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite delete_prompt_session connection")?;
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS prompt_sessions (
                        user_id INTEGER NOT NULL,
                        kind TEXT NOT NULL,
                        payload_json TEXT NOT NULL DEFAULT '',
                        created_at TEXT NOT NULL DEFAULT (datetime('now')),
                        expires_at TEXT NOT NULL,
                        PRIMARY KEY (user_id, kind)
                    );
                    CREATE INDEX IF NOT EXISTS idx_prompt_sessions_expires_at ON prompt_sessions(expires_at);",
                )
                .context("sqlite ensure prompt_sessions table")?;
                conn.execute(
                    "DELETE FROM prompt_sessions WHERE user_id = ?1 AND kind = ?2",
                    rusqlite::params![user_id, kind],
                )
                .context("sqlite delete_prompt_session")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("DELETE FROM prompt_sessions WHERE user_id = $1 AND kind = $2")
                    .bind(user_id)
                    .bind(kind)
                    .execute(pg_pool)
                    .await
                    .context("postgres delete_prompt_session")?;
                Ok(())
            }
        }
    }

    pub async fn upsert_preview_link_message(
        &self,
        user_id: i64,
        url: &str,
        original_message_id: i32,
        ttl_secs: i64,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite upsert_preview_link_message connection")?;
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS preview_contexts (
                        user_id INTEGER NOT NULL,
                        url TEXT NOT NULL,
                        original_message_id INTEGER,
                        time_range_start TEXT,
                        time_range_end TEXT,
                        burn_sub_lang TEXT,
                        audio_lang TEXT,
                        created_at TEXT NOT NULL DEFAULT (datetime('now')),
                        expires_at TEXT NOT NULL,
                        PRIMARY KEY (user_id, url)
                    );
                    CREATE INDEX IF NOT EXISTS idx_preview_contexts_expires_at ON preview_contexts(expires_at);",
                )
                .context("sqlite ensure preview_contexts table")?;
                conn.execute(
                    "INSERT INTO preview_contexts (
                        user_id, url, original_message_id, created_at, expires_at
                     ) VALUES (?1, ?2, ?3, datetime('now'), datetime('now', '+' || ?4 || ' seconds'))
                     ON CONFLICT(user_id, url) DO UPDATE SET
                        original_message_id = excluded.original_message_id,
                        expires_at = excluded.expires_at",
                    rusqlite::params![user_id, url, original_message_id, ttl_secs],
                )
                .context("sqlite upsert_preview_link_message")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO preview_contexts (
                        user_id, url, original_message_id, created_at, expires_at
                     ) VALUES ($1, $2, $3, NOW(), NOW() + ($4 * INTERVAL '1 second'))
                     ON CONFLICT (user_id, url) DO UPDATE SET
                        original_message_id = EXCLUDED.original_message_id,
                        expires_at = EXCLUDED.expires_at",
                )
                .bind(user_id)
                .bind(url)
                .bind(original_message_id)
                .bind(ttl_secs)
                .execute(pg_pool)
                .await
                .context("postgres upsert_preview_link_message")?;
                Ok(())
            }
        }
    }

    pub async fn upsert_preview_time_range(
        &self,
        user_id: i64,
        url: &str,
        start: &str,
        end: &str,
        ttl_secs: i64,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite upsert_preview_time_range connection")?;
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS preview_contexts (
                        user_id INTEGER NOT NULL,
                        url TEXT NOT NULL,
                        original_message_id INTEGER,
                        time_range_start TEXT,
                        time_range_end TEXT,
                        burn_sub_lang TEXT,
                        audio_lang TEXT,
                        created_at TEXT NOT NULL DEFAULT (datetime('now')),
                        expires_at TEXT NOT NULL,
                        PRIMARY KEY (user_id, url)
                    );
                    CREATE INDEX IF NOT EXISTS idx_preview_contexts_expires_at ON preview_contexts(expires_at);",
                )
                .context("sqlite ensure preview_contexts table")?;
                conn.execute(
                    "INSERT INTO preview_contexts (
                        user_id, url, time_range_start, time_range_end, created_at, expires_at
                     ) VALUES (?1, ?2, ?3, ?4, datetime('now'), datetime('now', '+' || ?5 || ' seconds'))
                     ON CONFLICT(user_id, url) DO UPDATE SET
                        time_range_start = excluded.time_range_start,
                        time_range_end = excluded.time_range_end,
                        expires_at = excluded.expires_at",
                    rusqlite::params![user_id, url, start, end, ttl_secs],
                )
                .context("sqlite upsert_preview_time_range")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO preview_contexts (
                        user_id, url, time_range_start, time_range_end, created_at, expires_at
                     ) VALUES ($1, $2, $3, $4, NOW(), NOW() + ($5 * INTERVAL '1 second'))
                     ON CONFLICT (user_id, url) DO UPDATE SET
                        time_range_start = EXCLUDED.time_range_start,
                        time_range_end = EXCLUDED.time_range_end,
                        expires_at = EXCLUDED.expires_at",
                )
                .bind(user_id)
                .bind(url)
                .bind(start)
                .bind(end)
                .bind(ttl_secs)
                .execute(pg_pool)
                .await
                .context("postgres upsert_preview_time_range")?;
                Ok(())
            }
        }
    }

    pub async fn set_preview_burn_sub_lang(
        &self,
        user_id: i64,
        url: &str,
        burn_sub_lang: Option<&str>,
        ttl_secs: i64,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite set_preview_burn_sub_lang connection")?;
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS preview_contexts (
                        user_id INTEGER NOT NULL,
                        url TEXT NOT NULL,
                        original_message_id INTEGER,
                        time_range_start TEXT,
                        time_range_end TEXT,
                        burn_sub_lang TEXT,
                        audio_lang TEXT,
                        created_at TEXT NOT NULL DEFAULT (datetime('now')),
                        expires_at TEXT NOT NULL,
                        PRIMARY KEY (user_id, url)
                    );
                    CREATE INDEX IF NOT EXISTS idx_preview_contexts_expires_at ON preview_contexts(expires_at);",
                )
                .context("sqlite ensure preview_contexts table")?;
                conn.execute(
                    "INSERT INTO preview_contexts (
                        user_id, url, burn_sub_lang, created_at, expires_at
                     ) VALUES (?1, ?2, ?3, datetime('now'), datetime('now', '+' || ?4 || ' seconds'))
                     ON CONFLICT(user_id, url) DO UPDATE SET
                        burn_sub_lang = excluded.burn_sub_lang,
                        expires_at = excluded.expires_at",
                    rusqlite::params![user_id, url, burn_sub_lang, ttl_secs],
                )
                .context("sqlite set_preview_burn_sub_lang")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO preview_contexts (
                        user_id, url, burn_sub_lang, created_at, expires_at
                     ) VALUES ($1, $2, $3, NOW(), NOW() + ($4 * INTERVAL '1 second'))
                     ON CONFLICT (user_id, url) DO UPDATE SET
                        burn_sub_lang = EXCLUDED.burn_sub_lang,
                        expires_at = EXCLUDED.expires_at",
                )
                .bind(user_id)
                .bind(url)
                .bind(burn_sub_lang)
                .bind(ttl_secs)
                .execute(pg_pool)
                .await
                .context("postgres set_preview_burn_sub_lang")?;
                Ok(())
            }
        }
    }

    pub async fn set_preview_audio_lang(
        &self,
        user_id: i64,
        url: &str,
        audio_lang: Option<&str>,
        ttl_secs: i64,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite set_preview_audio_lang connection")?;
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS preview_contexts (
                        user_id INTEGER NOT NULL,
                        url TEXT NOT NULL,
                        original_message_id INTEGER,
                        time_range_start TEXT,
                        time_range_end TEXT,
                        burn_sub_lang TEXT,
                        audio_lang TEXT,
                        created_at TEXT NOT NULL DEFAULT (datetime('now')),
                        expires_at TEXT NOT NULL,
                        PRIMARY KEY (user_id, url)
                    );
                    CREATE INDEX IF NOT EXISTS idx_preview_contexts_expires_at ON preview_contexts(expires_at);",
                )
                .context("sqlite ensure preview_contexts table")?;
                conn.execute(
                    "INSERT INTO preview_contexts (
                        user_id, url, audio_lang, created_at, expires_at
                     ) VALUES (?1, ?2, ?3, datetime('now'), datetime('now', '+' || ?4 || ' seconds'))
                     ON CONFLICT(user_id, url) DO UPDATE SET
                        audio_lang = excluded.audio_lang,
                        expires_at = excluded.expires_at",
                    rusqlite::params![user_id, url, audio_lang, ttl_secs],
                )
                .context("sqlite set_preview_audio_lang")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO preview_contexts (
                        user_id, url, audio_lang, created_at, expires_at
                     ) VALUES ($1, $2, $3, NOW(), NOW() + ($4 * INTERVAL '1 second'))
                     ON CONFLICT (user_id, url) DO UPDATE SET
                        audio_lang = EXCLUDED.audio_lang,
                        expires_at = EXCLUDED.expires_at",
                )
                .bind(user_id)
                .bind(url)
                .bind(audio_lang)
                .bind(ttl_secs)
                .execute(pg_pool)
                .await
                .context("postgres set_preview_audio_lang")?;
                Ok(())
            }
        }
    }

    pub async fn get_preview_context(&self, user_id: i64, url: &str) -> Result<Option<PreviewContext>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_preview_context connection")?;
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS preview_contexts (
                        user_id INTEGER NOT NULL,
                        url TEXT NOT NULL,
                        original_message_id INTEGER,
                        time_range_start TEXT,
                        time_range_end TEXT,
                        burn_sub_lang TEXT,
                        audio_lang TEXT,
                        created_at TEXT NOT NULL DEFAULT (datetime('now')),
                        expires_at TEXT NOT NULL,
                        PRIMARY KEY (user_id, url)
                    );
                    CREATE INDEX IF NOT EXISTS idx_preview_contexts_expires_at ON preview_contexts(expires_at);",
                )
                .context("sqlite ensure preview_contexts table")?;
                let row = conn
                    .query_row(
                        "SELECT original_message_id, time_range_start, time_range_end, burn_sub_lang, audio_lang
                         FROM preview_contexts
                         WHERE user_id = ?1
                           AND url = ?2
                           AND expires_at > datetime('now')",
                        rusqlite::params![user_id, url],
                        |row| {
                            let start: Option<String> = row.get(1)?;
                            let end: Option<String> = row.get(2)?;
                            Ok(PreviewContext {
                                original_message_id: row.get(0)?,
                                time_range: match (start, end) {
                                    (Some(start), Some(end)) => Some((start, end)),
                                    _ => None,
                                },
                                burn_sub_lang: row.get(3)?,
                                audio_lang: row.get(4)?,
                            })
                        },
                    )
                    .optional()
                    .context("sqlite get_preview_context")?;
                Ok(row)
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT original_message_id, time_range_start, time_range_end, burn_sub_lang, audio_lang
                     FROM preview_contexts
                     WHERE user_id = $1
                       AND url = $2
                       AND expires_at > NOW()",
                )
                .bind(user_id)
                .bind(url)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_preview_context")?;
                Ok(row.map(|row| PreviewContext {
                    original_message_id: row.get("original_message_id"),
                    time_range: match (
                        row.get::<Option<String>, _>("time_range_start"),
                        row.get::<Option<String>, _>("time_range_end"),
                    ) {
                        (Some(start), Some(end)) => Some((start, end)),
                        _ => None,
                    },
                    burn_sub_lang: row.get("burn_sub_lang"),
                    audio_lang: row.get("audio_lang"),
                }))
            }
        }
    }
}
