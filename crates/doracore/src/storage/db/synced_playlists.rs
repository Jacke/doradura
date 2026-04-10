//! Database operations for synced (imported) playlists from external platforms.

use super::DbConnection;
use rusqlite::Result;

// ==================== Structs ====================

#[derive(Debug, Clone)]
pub struct SyncedPlaylist {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub source_url: String,
    pub source_platform: String,
    pub track_count: i32,
    pub matched_count: i32,
    pub not_found_count: i32,
    pub sync_enabled: bool,
    pub last_synced_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SyncedTrack {
    pub id: i64,
    pub playlist_id: i64,
    pub position: i32,
    pub title: String,
    pub artist: Option<String>,
    pub duration_secs: Option<i32>,
    pub external_id: Option<String>,
    pub source_url: Option<String>,
    pub resolved_url: Option<String>,
    pub import_status: String,
    pub file_id: Option<String>,
    pub added_at: String,
}

// ==================== SyncedPlaylist CRUD ====================

pub fn create_synced_playlist(
    conn: &DbConnection,
    user_id: i64,
    name: &str,
    description: Option<&str>,
    source_url: &str,
    source_platform: &str,
    track_count: i32,
    matched_count: i32,
    not_found_count: i32,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO synced_playlists (user_id, name, description, source_url, source_platform, track_count, matched_count, not_found_count)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![user_id, name, description, source_url, source_platform, track_count, matched_count, not_found_count],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_synced_playlist(conn: &DbConnection, playlist_id: i64) -> Result<Option<SyncedPlaylist>> {
    let result = conn.query_row(
        "SELECT id, user_id, name, description, source_url, source_platform, track_count, matched_count, not_found_count, sync_enabled, last_synced_at, created_at, updated_at
         FROM synced_playlists WHERE id = ?1",
        [playlist_id],
        |row| {
            Ok(SyncedPlaylist {
                id: row.get(0)?,
                user_id: row.get(1)?,
                name: row.get(2)?,
                description: row.get(3)?,
                source_url: row.get(4)?,
                source_platform: row.get(5)?,
                track_count: row.get(6)?,
                matched_count: row.get(7)?,
                not_found_count: row.get(8)?,
                sync_enabled: row.get::<_, i32>(9)? != 0,
                last_synced_at: row.get(10)?,
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
            })
        },
    );
    match result {
        Ok(pl) => Ok(Some(pl)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn get_user_synced_playlists(conn: &DbConnection, user_id: i64) -> Result<Vec<SyncedPlaylist>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, name, description, source_url, source_platform, track_count, matched_count, not_found_count, sync_enabled, last_synced_at, created_at, updated_at
         FROM synced_playlists WHERE user_id = ?1 ORDER BY created_at DESC",
    )?;
    let rows = stmt.query_map([user_id], |row| {
        Ok(SyncedPlaylist {
            id: row.get(0)?,
            user_id: row.get(1)?,
            name: row.get(2)?,
            description: row.get(3)?,
            source_url: row.get(4)?,
            source_platform: row.get(5)?,
            track_count: row.get(6)?,
            matched_count: row.get(7)?,
            not_found_count: row.get(8)?,
            sync_enabled: row.get::<_, i32>(9)? != 0,
            last_synced_at: row.get(10)?,
            created_at: row.get(11)?,
            updated_at: row.get(12)?,
        })
    })?;
    rows.collect()
}

pub fn get_synced_playlist_by_url(
    conn: &DbConnection,
    user_id: i64,
    source_url: &str,
) -> Result<Option<SyncedPlaylist>> {
    let result = conn.query_row(
        "SELECT id, user_id, name, description, source_url, source_platform, track_count, matched_count, not_found_count, sync_enabled, last_synced_at, created_at, updated_at
         FROM synced_playlists WHERE user_id = ?1 AND source_url = ?2",
        rusqlite::params![user_id, source_url],
        |row| {
            Ok(SyncedPlaylist {
                id: row.get(0)?,
                user_id: row.get(1)?,
                name: row.get(2)?,
                description: row.get(3)?,
                source_url: row.get(4)?,
                source_platform: row.get(5)?,
                track_count: row.get(6)?,
                matched_count: row.get(7)?,
                not_found_count: row.get(8)?,
                sync_enabled: row.get::<_, i32>(9)? != 0,
                last_synced_at: row.get(10)?,
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
            })
        },
    );
    match result {
        Ok(pl) => Ok(Some(pl)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn count_user_synced_playlists(conn: &DbConnection, user_id: i64) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM synced_playlists WHERE user_id = ?1",
        [user_id],
        |row| row.get(0),
    )
}

pub fn update_synced_playlist_counts(
    conn: &DbConnection,
    playlist_id: i64,
    track_count: i32,
    matched_count: i32,
    not_found_count: i32,
) -> Result<()> {
    conn.execute(
        "UPDATE synced_playlists SET track_count = ?1, matched_count = ?2, not_found_count = ?3, last_synced_at = datetime('now'), updated_at = datetime('now') WHERE id = ?4",
        rusqlite::params![track_count, matched_count, not_found_count, playlist_id],
    )?;
    Ok(())
}

pub fn delete_synced_playlist(conn: &DbConnection, playlist_id: i64) -> Result<()> {
    // Delete tracks first (no CASCADE in schema)
    delete_synced_tracks(conn, playlist_id)?;
    conn.execute("DELETE FROM synced_playlists WHERE id = ?1", [playlist_id])?;
    Ok(())
}

/// Atomically increment matched_count and decrement not_found_count.
pub fn increment_synced_playlist_matched(conn: &DbConnection, playlist_id: i64, delta: i32) -> Result<()> {
    conn.execute(
        "UPDATE synced_playlists SET matched_count = matched_count + ?1, not_found_count = not_found_count - ?1, updated_at = datetime('now') WHERE id = ?2",
        rusqlite::params![delta, playlist_id],
    )?;
    Ok(())
}

// ==================== SyncedTrack CRUD ====================

pub fn add_synced_track(
    conn: &DbConnection,
    playlist_id: i64,
    position: i32,
    title: &str,
    artist: Option<&str>,
    duration_secs: Option<i32>,
    external_id: Option<&str>,
    source_url: Option<&str>,
    resolved_url: Option<&str>,
    import_status: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO synced_tracks (playlist_id, position, title, artist, duration_secs, external_id, source_url, resolved_url, import_status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![playlist_id, position, title, artist, duration_secs, external_id, source_url, resolved_url, import_status],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_synced_tracks(conn: &DbConnection, playlist_id: i64) -> Result<Vec<SyncedTrack>> {
    let mut stmt = conn.prepare(
        "SELECT id, playlist_id, position, title, artist, duration_secs, external_id, source_url, resolved_url, import_status, file_id, added_at
         FROM synced_tracks WHERE playlist_id = ?1 ORDER BY position ASC",
    )?;
    let rows = stmt.query_map([playlist_id], |row| {
        Ok(SyncedTrack {
            id: row.get(0)?,
            playlist_id: row.get(1)?,
            position: row.get(2)?,
            title: row.get(3)?,
            artist: row.get(4)?,
            duration_secs: row.get(5)?,
            external_id: row.get(6)?,
            source_url: row.get(7)?,
            resolved_url: row.get(8)?,
            import_status: row.get(9)?,
            file_id: row.get(10)?,
            added_at: row.get(11)?,
        })
    })?;
    rows.collect()
}

pub fn get_synced_tracks_page(
    conn: &DbConnection,
    playlist_id: i64,
    offset: i64,
    limit: i64,
) -> Result<Vec<SyncedTrack>> {
    let mut stmt = conn.prepare(
        "SELECT id, playlist_id, position, title, artist, duration_secs, external_id, source_url, resolved_url, import_status, file_id, added_at
         FROM synced_tracks WHERE playlist_id = ?1 ORDER BY position ASC LIMIT ?2 OFFSET ?3",
    )?;
    let rows = stmt.query_map(rusqlite::params![playlist_id, limit, offset], |row| {
        Ok(SyncedTrack {
            id: row.get(0)?,
            playlist_id: row.get(1)?,
            position: row.get(2)?,
            title: row.get(3)?,
            artist: row.get(4)?,
            duration_secs: row.get(5)?,
            external_id: row.get(6)?,
            source_url: row.get(7)?,
            resolved_url: row.get(8)?,
            import_status: row.get(9)?,
            file_id: row.get(10)?,
            added_at: row.get(11)?,
        })
    })?;
    rows.collect()
}

pub fn get_synced_track(conn: &DbConnection, track_id: i64) -> Result<Option<SyncedTrack>> {
    let result = conn.query_row(
        "SELECT id, playlist_id, position, title, artist, duration_secs, external_id, source_url, resolved_url, import_status, file_id, added_at
         FROM synced_tracks WHERE id = ?1",
        [track_id],
        |row| {
            Ok(SyncedTrack {
                id: row.get(0)?,
                playlist_id: row.get(1)?,
                position: row.get(2)?,
                title: row.get(3)?,
                artist: row.get(4)?,
                duration_secs: row.get(5)?,
                external_id: row.get(6)?,
                source_url: row.get(7)?,
                resolved_url: row.get(8)?,
                import_status: row.get(9)?,
                file_id: row.get(10)?,
                added_at: row.get(11)?,
            })
        },
    );
    match result {
        Ok(t) => Ok(Some(t)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn update_synced_track_file_id(conn: &DbConnection, track_id: i64, file_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE synced_tracks SET file_id = ?1 WHERE id = ?2",
        rusqlite::params![file_id, track_id],
    )?;
    Ok(())
}

pub fn update_synced_track_status(
    conn: &DbConnection,
    track_id: i64,
    status: &str,
    resolved_url: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE synced_tracks SET import_status = ?1, resolved_url = ?2 WHERE id = ?3",
        rusqlite::params![status, resolved_url, track_id],
    )?;
    Ok(())
}

pub fn delete_synced_tracks(conn: &DbConnection, playlist_id: i64) -> Result<()> {
    conn.execute("DELETE FROM synced_tracks WHERE playlist_id = ?1", [playlist_id])?;
    Ok(())
}

pub fn count_synced_tracks(conn: &DbConnection, playlist_id: i64) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM synced_tracks WHERE playlist_id = ?1",
        [playlist_id],
        |row| row.get(0),
    )
}
