//! Playlist, player session, and search cache database operations.

use super::DbConnection;
use rusqlite::Result;

// ==================== Playlist Structs ====================

/// A user playlist.
#[derive(Debug, Clone)]
pub struct Playlist {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub is_public: bool,
    pub share_token: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// An item in a playlist.
#[derive(Debug, Clone)]
pub struct PlaylistItem {
    pub id: i64,
    pub playlist_id: i64,
    pub position: i32,
    pub download_history_id: Option<i64>,
    pub title: String,
    pub artist: Option<String>,
    pub url: String,
    pub duration_secs: Option<i32>,
    pub file_id: Option<String>,
    pub source: String,
    pub added_at: String,
}

/// An active player session for a user.
#[derive(Debug, Clone)]
pub struct PlayerSession {
    pub user_id: i64,
    pub playlist_id: i64,
    pub current_position: i32,
    pub is_shuffle: bool,
    pub player_message_id: Option<i32>,
    pub sticker_message_id: Option<i32>,
    pub updated_at: String,
}

// ==================== Playlist CRUD ====================

/// Create a new playlist for a user.
pub fn create_playlist(conn: &DbConnection, user_id: i64, name: &str, description: Option<&str>) -> Result<i64> {
    conn.execute(
        "INSERT INTO playlists (user_id, name, description) VALUES (?1, ?2, ?3)",
        rusqlite::params![user_id, name, description],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Get a playlist by ID.
pub fn get_playlist(conn: &DbConnection, playlist_id: i64) -> Result<Option<Playlist>> {
    let result = conn.query_row(
        "SELECT id, user_id, name, description, is_public, share_token, created_at, updated_at
         FROM playlists WHERE id = ?1",
        [playlist_id],
        |row| {
            Ok(Playlist {
                id: row.get(0)?,
                user_id: row.get(1)?,
                name: row.get(2)?,
                description: row.get(3)?,
                is_public: row.get::<_, i32>(4)? != 0,
                share_token: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        },
    );
    match result {
        Ok(pl) => Ok(Some(pl)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Get all playlists for a user (ordered by updated_at desc).
pub fn get_user_playlists(conn: &DbConnection, user_id: i64) -> Result<Vec<Playlist>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, name, description, is_public, share_token, created_at, updated_at
         FROM playlists WHERE user_id = ?1 ORDER BY updated_at DESC",
    )?;
    let rows = stmt.query_map([user_id], |row| {
        Ok(Playlist {
            id: row.get(0)?,
            user_id: row.get(1)?,
            name: row.get(2)?,
            description: row.get(3)?,
            is_public: row.get::<_, i32>(4)? != 0,
            share_token: row.get(5)?,
            created_at: row.get(6)?,
            updated_at: row.get(7)?,
        })
    })?;
    rows.collect()
}

/// Update playlist name and/or description.
pub fn update_playlist(conn: &DbConnection, playlist_id: i64, name: &str, description: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE playlists SET name = ?1, description = ?2, updated_at = datetime('now') WHERE id = ?3",
        rusqlite::params![name, description, playlist_id],
    )?;
    Ok(())
}

/// Rename a playlist (preserves description).
pub fn rename_playlist(conn: &DbConnection, playlist_id: i64, name: &str) -> Result<()> {
    conn.execute(
        "UPDATE playlists SET name = ?1, updated_at = datetime('now') WHERE id = ?2",
        rusqlite::params![name, playlist_id],
    )?;
    Ok(())
}

/// Delete a playlist (cascade deletes items too).
pub fn delete_playlist(conn: &DbConnection, playlist_id: i64) -> Result<()> {
    conn.execute("DELETE FROM playlists WHERE id = ?1", [playlist_id])?;
    Ok(())
}

/// Count playlists for a user.
pub fn count_user_playlists(conn: &DbConnection, user_id: i64) -> Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM playlists WHERE user_id = ?1", [user_id], |row| {
        row.get(0)
    })
}

/// Set the share_token for a playlist.
pub fn set_playlist_share_token(conn: &DbConnection, playlist_id: i64, token: &str) -> Result<()> {
    conn.execute(
        "UPDATE playlists SET share_token = ?1, updated_at = datetime('now') WHERE id = ?2",
        rusqlite::params![token, playlist_id],
    )?;
    Ok(())
}

/// Toggle public visibility of a playlist.
pub fn set_playlist_public(conn: &DbConnection, playlist_id: i64, is_public: bool) -> Result<()> {
    conn.execute(
        "UPDATE playlists SET is_public = ?1, updated_at = datetime('now') WHERE id = ?2",
        rusqlite::params![is_public as i32, playlist_id],
    )?;
    Ok(())
}

/// Get a playlist by its share_token.
pub fn get_playlist_by_share_token(conn: &DbConnection, token: &str) -> Result<Option<Playlist>> {
    let result = conn.query_row(
        "SELECT id, user_id, name, description, is_public, share_token, created_at, updated_at
         FROM playlists WHERE share_token = ?1",
        [token],
        |row| {
            Ok(Playlist {
                id: row.get(0)?,
                user_id: row.get(1)?,
                name: row.get(2)?,
                description: row.get(3)?,
                is_public: row.get::<_, i32>(4)? != 0,
                share_token: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        },
    );
    match result {
        Ok(pl) => Ok(Some(pl)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

// ==================== Playlist Items ====================

/// Add an item to a playlist at the end (auto-increments position).
pub fn add_playlist_item(
    conn: &DbConnection,
    playlist_id: i64,
    title: &str,
    artist: Option<&str>,
    url: &str,
    duration_secs: Option<i32>,
    file_id: Option<&str>,
    source: &str,
) -> Result<i64> {
    let next_pos: i32 = conn.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM playlist_items WHERE playlist_id = ?1",
        [playlist_id],
        |row| row.get(0),
    )?;
    conn.execute(
        "INSERT INTO playlist_items (playlist_id, position, title, artist, url, duration_secs, file_id, source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            playlist_id,
            next_pos,
            title,
            artist,
            url,
            duration_secs,
            file_id,
            source
        ],
    )?;
    // Touch playlist updated_at
    conn.execute(
        "UPDATE playlists SET updated_at = datetime('now') WHERE id = ?1",
        [playlist_id],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Remove an item from a playlist by item ID.
pub fn remove_playlist_item(conn: &DbConnection, item_id: i64) -> Result<()> {
    // Get playlist_id before deleting for updated_at touch
    let playlist_id: Option<i64> = conn
        .query_row(
            "SELECT playlist_id FROM playlist_items WHERE id = ?1",
            [item_id],
            |row| row.get(0),
        )
        .ok();
    conn.execute("DELETE FROM playlist_items WHERE id = ?1", [item_id])?;
    if let Some(pl_id) = playlist_id {
        conn.execute(
            "UPDATE playlists SET updated_at = datetime('now') WHERE id = ?1",
            [pl_id],
        )?;
    }
    Ok(())
}

/// Reorder: move item up (direction = -1) or down (direction = 1).
pub fn reorder_playlist_item(conn: &DbConnection, item_id: i64, direction: i32) -> Result<()> {
    let (playlist_id, current_pos): (i64, i32) = conn.query_row(
        "SELECT playlist_id, position FROM playlist_items WHERE id = ?1",
        [item_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let new_pos = current_pos + direction;
    if new_pos < 0 {
        return Ok(());
    }
    // Swap positions atomically
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE playlist_items SET position = ?1 WHERE playlist_id = ?2 AND position = ?3",
        rusqlite::params![current_pos, playlist_id, new_pos],
    )?;
    tx.execute(
        "UPDATE playlist_items SET position = ?1 WHERE id = ?2",
        rusqlite::params![new_pos, item_id],
    )?;
    tx.commit()?;
    Ok(())
}

/// Get all items in a playlist ordered by position.
pub fn get_playlist_items(conn: &DbConnection, playlist_id: i64) -> Result<Vec<PlaylistItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, playlist_id, position, download_history_id, title, artist, url, duration_secs, file_id, source, added_at
         FROM playlist_items WHERE playlist_id = ?1 ORDER BY position",
    )?;
    let rows = stmt.query_map([playlist_id], |row| {
        Ok(PlaylistItem {
            id: row.get(0)?,
            playlist_id: row.get(1)?,
            position: row.get(2)?,
            download_history_id: row.get(3)?,
            title: row.get(4)?,
            artist: row.get(5)?,
            url: row.get(6)?,
            duration_secs: row.get(7)?,
            file_id: row.get(8)?,
            source: row.get(9)?,
            added_at: row.get(10)?,
        })
    })?;
    rows.collect()
}

/// Get a paginated slice of items from a playlist.
pub fn get_playlist_items_page(
    conn: &DbConnection,
    playlist_id: i64,
    offset: i64,
    limit: i64,
) -> Result<Vec<PlaylistItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, playlist_id, position, download_history_id, title, artist, url, duration_secs, file_id, source, added_at
         FROM playlist_items WHERE playlist_id = ?1 ORDER BY position LIMIT ?2 OFFSET ?3",
    )?;
    let rows = stmt.query_map(rusqlite::params![playlist_id, limit, offset], |row| {
        Ok(PlaylistItem {
            id: row.get(0)?,
            playlist_id: row.get(1)?,
            position: row.get(2)?,
            download_history_id: row.get(3)?,
            title: row.get(4)?,
            artist: row.get(5)?,
            url: row.get(6)?,
            duration_secs: row.get(7)?,
            file_id: row.get(8)?,
            source: row.get(9)?,
            added_at: row.get(10)?,
        })
    })?;
    rows.collect()
}

/// Get a single playlist item at a given position.
pub fn get_playlist_item_at_position(
    conn: &DbConnection,
    playlist_id: i64,
    position: i32,
) -> Result<Option<PlaylistItem>> {
    let result = conn.query_row(
        "SELECT id, playlist_id, position, download_history_id, title, artist, url, duration_secs, file_id, source, added_at
         FROM playlist_items WHERE playlist_id = ?1 AND position = ?2",
        rusqlite::params![playlist_id, position],
        |row| {
            Ok(PlaylistItem {
                id: row.get(0)?,
                playlist_id: row.get(1)?,
                position: row.get(2)?,
                download_history_id: row.get(3)?,
                title: row.get(4)?,
                artist: row.get(5)?,
                url: row.get(6)?,
                duration_secs: row.get(7)?,
                file_id: row.get(8)?,
                source: row.get(9)?,
                added_at: row.get(10)?,
            })
        },
    );
    match result {
        Ok(item) => Ok(Some(item)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Update the cached Telegram file_id for a playlist item.
pub fn update_item_file_id(conn: &DbConnection, item_id: i64, file_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE playlist_items SET file_id = ?1 WHERE id = ?2",
        rusqlite::params![file_id, item_id],
    )?;
    Ok(())
}

/// Count items in a playlist.
pub fn count_playlist_items(conn: &DbConnection, playlist_id: i64) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM playlist_items WHERE playlist_id = ?1",
        [playlist_id],
        |row| row.get(0),
    )
}

// ==================== Player Sessions ====================

/// Create or replace a player session for a user.
pub fn create_player_session(
    conn: &DbConnection,
    user_id: i64,
    playlist_id: i64,
    player_message_id: Option<i32>,
    sticker_message_id: Option<i32>,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO player_sessions (user_id, playlist_id, current_position, is_shuffle, player_message_id, sticker_message_id, updated_at)
         VALUES (?1, ?2, 0, 0, ?3, ?4, datetime('now'))",
        rusqlite::params![user_id, playlist_id, player_message_id, sticker_message_id],
    )?;
    Ok(())
}

/// Get the player session for a user.
pub fn get_player_session(conn: &DbConnection, user_id: i64) -> Result<Option<PlayerSession>> {
    let result = conn.query_row(
        "SELECT user_id, playlist_id, current_position, is_shuffle, player_message_id, sticker_message_id, updated_at
         FROM player_sessions WHERE user_id = ?1",
        [user_id],
        |row| {
            Ok(PlayerSession {
                user_id: row.get(0)?,
                playlist_id: row.get(1)?,
                current_position: row.get(2)?,
                is_shuffle: row.get::<_, i32>(3)? != 0,
                player_message_id: row.get(4)?,
                sticker_message_id: row.get(5)?,
                updated_at: row.get(6)?,
            })
        },
    );
    match result {
        Ok(session) => Ok(Some(session)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Update current playback position and optionally the player message ID.
pub fn update_player_position(
    conn: &DbConnection,
    user_id: i64,
    position: i32,
    player_message_id: Option<i32>,
) -> Result<()> {
    if let Some(msg_id) = player_message_id {
        conn.execute(
            "UPDATE player_sessions SET current_position = ?1, player_message_id = ?2, updated_at = datetime('now') WHERE user_id = ?3",
            rusqlite::params![position, msg_id, user_id],
        )?;
    } else {
        conn.execute(
            "UPDATE player_sessions SET current_position = ?1, updated_at = datetime('now') WHERE user_id = ?2",
            rusqlite::params![position, user_id],
        )?;
    }
    Ok(())
}

/// Toggle shuffle mode.
pub fn toggle_player_shuffle(conn: &DbConnection, user_id: i64) -> Result<bool> {
    let current: i32 = conn.query_row(
        "SELECT is_shuffle FROM player_sessions WHERE user_id = ?1",
        [user_id],
        |row| row.get(0),
    )?;
    let new_val = if current == 0 { 1 } else { 0 };
    conn.execute(
        "UPDATE player_sessions SET is_shuffle = ?1, updated_at = datetime('now') WHERE user_id = ?2",
        rusqlite::params![new_val, user_id],
    )?;
    Ok(new_val != 0)
}

/// Delete a player session.
pub fn delete_player_session(conn: &DbConnection, user_id: i64) -> Result<()> {
    conn.execute("DELETE FROM player_sessions WHERE user_id = ?1", [user_id])?;
    Ok(())
}

// ==================== Search Cache ====================

/// Cache search results (upsert).
pub fn cache_search_results(conn: &DbConnection, query_key: &str, results_json: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO search_cache (query_key, results_json, created_at) VALUES (?1, ?2, datetime('now'))",
        rusqlite::params![query_key, results_json],
    )?;
    Ok(())
}

/// Get cached search results (returns None if not found or expired).
pub fn get_cached_search(conn: &DbConnection, query_key: &str, max_age_minutes: i64) -> Result<Option<String>> {
    let result = conn.query_row(
        "SELECT results_json FROM search_cache
         WHERE query_key = ?1
         AND datetime(created_at, '+' || ?2 || ' minutes') > datetime('now')",
        rusqlite::params![query_key, max_age_minutes],
        |row| row.get(0),
    );
    match result {
        Ok(json) => Ok(Some(json)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Delete expired cache entries.
pub fn cleanup_search_cache(conn: &DbConnection, max_age_minutes: i64) -> Result<usize> {
    let deleted = conn.execute(
        "DELETE FROM search_cache WHERE datetime(created_at, '+' || ?1 || ' minutes') <= datetime('now')",
        rusqlite::params![max_age_minutes],
    )?;
    Ok(deleted)
}

// ==================== Player Messages (UI tracking for cleanup) ====================

/// Track a bot UI message for cleanup when player exits.
pub fn add_player_message(conn: &DbConnection, user_id: i64, message_id: i32) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO player_messages (user_id, message_id) VALUES (?1, ?2)",
        rusqlite::params![user_id, message_id],
    )?;
    Ok(())
}

/// Get all tracked UI message IDs for a user's player session.
pub fn get_player_messages(conn: &DbConnection, user_id: i64) -> Result<Vec<i32>> {
    let mut stmt = conn.prepare("SELECT message_id FROM player_messages WHERE user_id = ?1")?;
    let rows = stmt.query_map([user_id], |row| row.get(0))?;
    rows.collect()
}

/// Delete all tracked UI messages for a user (after cleanup).
pub fn delete_player_messages(conn: &DbConnection, user_id: i64) -> Result<()> {
    conn.execute("DELETE FROM player_messages WHERE user_id = ?1", [user_id])?;
    Ok(())
}

/// Update the sticker message ID on a player session.
pub fn update_player_sticker_id(conn: &DbConnection, user_id: i64, sticker_message_id: i32) -> Result<()> {
    conn.execute(
        "UPDATE player_sessions SET sticker_message_id = ?1, updated_at = datetime('now') WHERE user_id = ?2",
        rusqlite::params![sticker_message_id, user_id],
    )?;
    Ok(())
}
