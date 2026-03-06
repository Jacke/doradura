//! Database operations for user vault (private Telegram channel storage).

use super::DbConnection;
use rusqlite::Result;

#[derive(Debug, Clone)]
pub struct UserVault {
    pub user_id: i64,
    pub channel_id: i64,
    pub channel_title: Option<String>,
    pub is_active: bool,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct VaultCacheEntry {
    pub id: i64,
    pub user_id: i64,
    pub url: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub duration_secs: Option<i32>,
    pub file_id: String,
    pub message_id: Option<i64>,
    pub file_size: Option<i64>,
    pub created_at: String,
}

pub fn get_user_vault(conn: &DbConnection, user_id: i64) -> Result<Option<UserVault>> {
    let result = conn.query_row(
        "SELECT user_id, channel_id, channel_title, is_active, created_at FROM user_vaults WHERE user_id = ?1",
        [user_id],
        |row| {
            Ok(UserVault {
                user_id: row.get(0)?,
                channel_id: row.get(1)?,
                channel_title: row.get(2)?,
                is_active: row.get::<_, i32>(3)? != 0,
                created_at: row.get(4)?,
            })
        },
    );
    match result {
        Ok(v) => Ok(Some(v)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn set_user_vault(conn: &DbConnection, user_id: i64, channel_id: i64, channel_title: Option<&str>) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO user_vaults (user_id, channel_id, channel_title, is_active, created_at, updated_at)
         VALUES (?1, ?2, ?3, 1, COALESCE((SELECT created_at FROM user_vaults WHERE user_id = ?1), datetime('now')), datetime('now'))",
        rusqlite::params![user_id, channel_id, channel_title],
    )?;
    Ok(())
}

pub fn deactivate_user_vault(conn: &DbConnection, user_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE user_vaults SET is_active = 0, updated_at = datetime('now') WHERE user_id = ?1",
        [user_id],
    )?;
    Ok(())
}

pub fn activate_user_vault(conn: &DbConnection, user_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE user_vaults SET is_active = 1, updated_at = datetime('now') WHERE user_id = ?1",
        [user_id],
    )?;
    Ok(())
}

pub fn delete_user_vault(conn: &DbConnection, user_id: i64) -> Result<()> {
    conn.execute("DELETE FROM user_vaults WHERE user_id = ?1", [user_id])?;
    Ok(())
}

pub fn get_vault_cached_file_id(conn: &DbConnection, user_id: i64, url: &str) -> Option<String> {
    conn.query_row(
        "SELECT file_id FROM vault_cache WHERE user_id = ?1 AND url = ?2",
        rusqlite::params![user_id, url],
        |row| row.get(0),
    )
    .ok()
}

pub fn save_vault_cache_entry(
    conn: &DbConnection,
    user_id: i64,
    url: &str,
    title: Option<&str>,
    artist: Option<&str>,
    duration_secs: Option<i32>,
    file_id: &str,
    message_id: Option<i64>,
    file_size: Option<i64>,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO vault_cache (user_id, url, title, artist, duration_secs, file_id, message_id, file_size)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![user_id, url, title, artist, duration_secs, file_id, message_id, file_size],
    )?;
    Ok(())
}

pub fn get_vault_cache_stats(conn: &DbConnection, user_id: i64) -> (i64, i64) {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM vault_cache WHERE user_id = ?1",
            [user_id],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let total_bytes: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(file_size), 0) FROM vault_cache WHERE user_id = ?1",
            [user_id],
            |row| row.get(0),
        )
        .unwrap_or(0);
    (count, total_bytes)
}

pub fn clear_vault_cache(conn: &DbConnection, user_id: i64) -> Result<()> {
    conn.execute("DELETE FROM vault_cache WHERE user_id = ?1", [user_id])?;
    Ok(())
}
