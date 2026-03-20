//! Download history, stats, and file cache operations.

use super::DbConnection;
use rusqlite::Result;

/// Structure representing a download history entry.
#[derive(Debug, Clone)]
pub struct DownloadHistoryEntry {
    /// Record ID
    pub id: i64,
    /// URL of the downloaded content
    pub url: String,
    /// Track/video title
    pub title: String,
    /// Download format (mp3, mp4, srt, txt)
    pub format: String,
    /// Download date and time
    pub downloaded_at: String,
    /// Telegram file_id (optional)
    pub file_id: Option<String>,
    /// Track/video author (optional)
    pub author: Option<String>,
    /// File size in bytes (optional)
    pub file_size: Option<i64>,
    /// Duration in seconds (optional)
    pub duration: Option<i64>,
    /// Video quality (optional, for mp4)
    pub video_quality: Option<String>,
    /// Audio bitrate (optional, for mp3)
    pub audio_bitrate: Option<String>,
    /// Bot API base URL used when saving this entry (optional, for debugging)
    pub bot_api_url: Option<String>,
    /// Whether a local Bot API server was used (0/1, optional for older rows)
    pub bot_api_is_local: Option<i64>,
    /// Source file ID (for split videos)
    pub source_id: Option<i64>,
    /// Part number (for split videos)
    pub part_index: Option<i32>,
    /// User-defined category name (optional)
    pub category: Option<String>,
}

fn current_bot_api_info() -> (Option<String>, i64) {
    let url = std::env::var("BOT_API_URL").ok();
    let is_local = url.as_deref().map(|u| !u.contains("api.telegram.org")).unwrap_or(false);
    (url, if is_local { 1 } else { 0 })
}

/// Saves an entry to the download history.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `url` - URL of the downloaded content
/// * `title` - Track/video title
/// * `format` - Download format (mp3, mp4, srt, txt)
/// * `file_id` - Telegram file_id, if content was sent to Telegram (optional)
/// * `author` - Track/video author (optional)
/// * `file_size` - File size in bytes (optional)
/// * `duration` - Duration in seconds (optional)
/// * `video_quality` - Video quality (optional)
/// * `audio_bitrate` - Audio bitrate (optional)
/// * `source_id` - Source file ID (for split videos)
/// * `part_index` - Part number (for split videos)
///
/// # Returns
///
/// Returns `Ok(id)` on success (ID of the inserted record) or a database error.
pub fn save_download_history(
    conn: &DbConnection,
    telegram_id: i64,
    url: &str,
    title: &str,
    format: &str,
    file_id: Option<&str>,
    author: Option<&str>,
    file_size: Option<i64>,
    duration: Option<i64>,
    video_quality: Option<&str>,
    audio_bitrate: Option<&str>,
    source_id: Option<i64>,
    part_index: Option<i32>,
) -> Result<i64> {
    let (bot_api_url, bot_api_is_local) = current_bot_api_info();
    conn.execute(
        "INSERT INTO download_history (
            user_id, url, title, format, file_id, author, file_size, duration, video_quality, audio_bitrate,
            bot_api_url, bot_api_is_local, source_id, part_index
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        rusqlite::params![
            telegram_id,
            url,
            title,
            format,
            file_id,
            author,
            file_size,
            duration,
            video_quality,
            audio_bitrate,
            bot_api_url,
            bot_api_is_local,
            source_id,
            part_index
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Returns the number of downloads completed by `telegram_id` since UTC midnight today.
///
/// Used to enforce `daily_download_limit` for the Free plan. Counts rows in
/// `download_history` whose `downloaded_at` timestamp falls within the current
/// UTC calendar day. This is intentionally a cheap `COUNT(*)` with no joins.
///
/// # Errors
///
/// Returns a rusqlite error only if the prepared statement or query itself fails,
/// which is extremely rare for a simple `COUNT` on an indexed column.
pub fn count_user_downloads_today(conn: &DbConnection, telegram_id: i64) -> Result<u32> {
    conn.query_row(
        "SELECT COUNT(*) FROM download_history \
         WHERE user_id = ?1 AND DATE(downloaded_at) = DATE('now')",
        [&telegram_id as &dyn rusqlite::ToSql],
        |row| row.get::<_, u32>(0),
    )
}

/// Finds a cached Telegram file_id for the given URL, format and quality/bitrate.
/// Searches across ALL users — file_ids are reusable within the same Bot API server.
/// Returns the most recent file_id that matches.
pub fn find_cached_file_id(
    conn: &DbConnection,
    url: &str,
    format: &str,
    video_quality: Option<&str>,
    audio_bitrate: Option<&str>,
) -> Result<Option<String>> {
    let (current_api_url, current_is_local) = current_bot_api_info();
    let mut stmt = conn.prepare(
        "SELECT file_id FROM download_history
         WHERE url = ?1 AND format = ?2 AND file_id IS NOT NULL
         AND bot_api_is_local = ?3
         AND (?4 IS NULL OR video_quality = ?4)
         AND (?5 IS NULL OR audio_bitrate = ?5)
         AND (?6 IS NULL OR bot_api_url = ?6)
         ORDER BY downloaded_at DESC LIMIT 1",
    )?;
    let result = stmt.query_row(
        rusqlite::params![
            url,
            format,
            current_is_local,
            video_quality,
            audio_bitrate,
            current_api_url
        ],
        |row| row.get::<_, String>(0),
    );
    match result {
        Ok(fid) => Ok(Some(fid)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Gets the last N download history entries for a user.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `limit` - Maximum number of records (default 20)
///
/// # Returns
///
/// Returns `Ok(Vec<DownloadHistoryEntry>)` with history records or a database error.
pub fn get_download_history(
    conn: &DbConnection,
    telegram_id: i64,
    limit: Option<i32>,
) -> Result<Vec<DownloadHistoryEntry>> {
    let limit = limit.unwrap_or(20);
    let mut stmt = conn.prepare(
        "SELECT id, url, title, format, downloaded_at, file_id, author, file_size, duration, video_quality, audio_bitrate,
                bot_api_url, bot_api_is_local, source_id, part_index, category
         FROM download_history
         WHERE user_id = ? ORDER BY downloaded_at DESC LIMIT ?",
    )?;
    let rows = stmt.query_map(rusqlite::params![telegram_id, limit], |row| {
        Ok(DownloadHistoryEntry {
            id: row.get(0)?,
            url: row.get(1)?,
            title: row.get(2)?,
            format: row.get(3)?,
            downloaded_at: row.get(4)?,
            file_id: row.get(5)?,
            author: row.get(6)?,
            file_size: row.get(7)?,
            duration: row.get(8)?,
            video_quality: row.get(9)?,
            audio_bitrate: row.get(10)?,
            bot_api_url: row.get(11)?,
            bot_api_is_local: row.get(12)?,
            source_id: row.get(13)?,
            part_index: row.get(14)?,
            category: row.get(15)?,
        })
    })?;

    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }
    Ok(entries)
}

/// Structure representing a file with file_id for the administrator.
#[derive(Debug, Clone)]
pub struct SentFile {
    /// Record ID
    pub id: i64,
    /// Telegram ID of the user
    pub user_id: i64,
    /// Username of the user (if available)
    pub username: Option<String>,
    /// URL of the downloaded content
    pub url: String,
    /// File title
    pub title: String,
    /// File format (mp3, mp4, srt, txt)
    pub format: String,
    /// Download date and time
    pub downloaded_at: String,
    /// Telegram file_id
    pub file_id: String,
    /// Telegram message_id (for MTProto refresh)
    pub message_id: Option<i32>,
    /// Chat ID where message was sent
    pub chat_id: Option<i64>,
}

/// Gets the list of files with file_id for the administrator.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `limit` - Maximum number of records (default 50)
///
/// # Returns
///
/// Returns `Ok(Vec<SentFile>)` with file records or a database error.
/// Returns only files that have a file_id.
pub fn get_sent_files(conn: &DbConnection, limit: Option<i32>) -> Result<Vec<SentFile>> {
    let limit = limit.unwrap_or(50);
    let mut stmt = conn.prepare(
        "SELECT dh.id, dh.user_id, u.username, dh.url, dh.title, dh.format, dh.downloaded_at, dh.file_id,
                dh.message_id, dh.chat_id
         FROM download_history dh
         LEFT JOIN users u ON dh.user_id = u.telegram_id
         WHERE dh.file_id IS NOT NULL
         ORDER BY dh.downloaded_at DESC
         LIMIT ?",
    )?;
    let rows = stmt.query_map([limit], |row| {
        Ok(SentFile {
            id: row.get(0)?,
            user_id: row.get(1)?,
            username: row.get(2)?,
            url: row.get(3)?,
            title: row.get(4)?,
            format: row.get(5)?,
            downloaded_at: row.get(6)?,
            file_id: row.get(7)?,
            message_id: row.get(8)?,
            chat_id: row.get(9)?,
        })
    })?;

    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }
    Ok(entries)
}

/// Deletes an entry from the download history.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `entry_id` - ID of the record to delete
///
/// # Returns
///
/// Returns `Ok(true)` if the record was deleted, `Ok(false)` if not found,
/// or a database error.
pub fn delete_download_history_entry(conn: &DbConnection, telegram_id: i64, entry_id: i64) -> Result<bool> {
    let rows_affected = conn.execute(
        "DELETE FROM download_history WHERE id = ?1 AND user_id = ?2",
        [&entry_id as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(rows_affected > 0)
}

/// Gets a download history entry by ID.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `entry_id` - Record ID
///
/// # Returns
///
/// Returns `Ok(Some(DownloadHistoryEntry))` if found, `Ok(None)` if not found,
/// or a database error.
pub fn get_download_history_entry(
    conn: &DbConnection,
    telegram_id: i64,
    entry_id: i64,
) -> Result<Option<DownloadHistoryEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, url, title, format, downloaded_at, file_id, author, file_size, duration, video_quality, audio_bitrate,
                bot_api_url, bot_api_is_local, source_id, part_index, category
         FROM download_history
         WHERE id = ?1 AND user_id = ?2",
    )?;
    let mut rows = stmt.query_map(rusqlite::params![entry_id, telegram_id], |row| {
        Ok(DownloadHistoryEntry {
            id: row.get(0)?,
            url: row.get(1)?,
            title: row.get(2)?,
            format: row.get(3)?,
            downloaded_at: row.get(4)?,
            file_id: row.get(5)?,
            author: row.get(6)?,
            file_size: row.get(7)?,
            duration: row.get(8)?,
            video_quality: row.get(9)?,
            audio_bitrate: row.get(10)?,
            bot_api_url: row.get(11)?,
            bot_api_is_local: row.get(12)?,
            source_id: row.get(13)?,
            part_index: row.get(14)?,
            category: row.get(15)?,
        })
    })?;

    if let Some(row) = rows.next() {
        Ok(Some(row?))
    } else {
        Ok(None)
    }
}

/// User statistics structure
#[derive(Debug, Clone)]
pub struct UserStats {
    pub total_downloads: i64,
    pub total_size: i64, // in bytes (approximate)
    pub active_days: i64,
    pub top_artists: Vec<(String, i64)>,     // (artist, count)
    pub top_formats: Vec<(String, i64)>,     // (format, count)
    pub activity_by_day: Vec<(String, i64)>, // (date, count) for the last 7 days
}

/// Gets user statistics
pub fn get_user_stats(conn: &DbConnection, telegram_id: i64) -> Result<UserStats> {
    // Total download count
    let total_downloads: i64 = conn.query_row(
        "SELECT COUNT(*) FROM download_history WHERE user_id = ?",
        [&telegram_id as &dyn rusqlite::ToSql],
        |row| row.get(0),
    )?;

    // Approximate total size (rough estimate: mp3 ~5MB, mp4 ~50MB)
    let total_size: i64 = match conn.query_row(
        "SELECT
            SUM(CASE
                WHEN format = 'mp3' THEN 5000000
                WHEN format = 'mp4' THEN 50000000
                ELSE 1000000
            END)
        FROM download_history WHERE user_id = ?",
        [&telegram_id as &dyn rusqlite::ToSql],
        |row| row.get::<_, Option<i64>>(0),
    ) {
        Ok(Some(size)) => size,
        Ok(None) => 0,
        Err(e) => return Err(e),
    };

    // Number of active days
    let active_days: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT DATE(downloaded_at)) FROM download_history WHERE user_id = ?",
        [&telegram_id as &dyn rusqlite::ToSql],
        |row| row.get(0),
    )?;

    // Top-5 artists (parsed from title: "Artist - Song")
    let mut stmt =
        conn.prepare("SELECT title FROM download_history WHERE user_id = ? ORDER BY downloaded_at DESC LIMIT 100")?;
    let rows = stmt.query_map([&telegram_id as &dyn rusqlite::ToSql], |row| row.get::<_, String>(0))?;

    let mut artist_counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for row in rows {
        if let Ok(title) = row {
            // Try to extract artist from "Artist - Song" format
            if let Some(pos) = title.find(" - ") {
                let artist = title[..pos].trim().to_string();
                if !artist.is_empty() {
                    *artist_counts.entry(artist).or_insert(0) += 1;
                }
            }
        }
    }

    let mut top_artists: Vec<(String, i64)> = artist_counts.into_iter().collect();
    top_artists.sort_by(|a, b| b.1.cmp(&a.1));
    top_artists.truncate(5);

    // Top formats
    let mut stmt = conn.prepare(
        "SELECT format, COUNT(*) as cnt FROM download_history
         WHERE user_id = ? GROUP BY format ORDER BY cnt DESC LIMIT 5",
    )?;
    let rows = stmt.query_map([&telegram_id as &dyn rusqlite::ToSql], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;

    let mut top_formats = Vec::new();
    for row in rows {
        if let Ok((format, count)) = row {
            top_formats.push((format, count));
        }
    }

    // Activity by day (last 7 days)
    let mut stmt = conn.prepare(
        "SELECT DATE(downloaded_at) as day, COUNT(*) as cnt
         FROM download_history
         WHERE user_id = ? AND downloaded_at >= datetime('now', '-7 days')
         GROUP BY DATE(downloaded_at)
         ORDER BY day DESC",
    )?;
    let rows = stmt.query_map([&telegram_id as &dyn rusqlite::ToSql], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;

    let mut activity_by_day = Vec::new();
    for row in rows {
        if let Ok((day, count)) = row {
            activity_by_day.push((day, count));
        }
    }

    Ok(UserStats {
        total_downloads,
        total_size,
        active_days,
        top_artists,
        top_formats,
        activity_by_day,
    })
}

/// Global statistics structure
#[derive(Debug, Clone)]
pub struct GlobalStats {
    pub total_users: i64,
    pub total_downloads: i64,
    pub top_tracks: Vec<(String, i64)>,  // (title, count)
    pub top_formats: Vec<(String, i64)>, // (format, count)
}

/// Gets global bot statistics
pub fn get_global_stats(conn: &DbConnection) -> Result<GlobalStats> {
    // Total number of users
    let total_users: i64 = conn.query_row("SELECT COUNT(DISTINCT user_id) FROM download_history", [], |row| {
        row.get(0)
    })?;

    // Total number of downloads
    let total_downloads: i64 = conn.query_row("SELECT COUNT(*) FROM download_history", [], |row| row.get(0))?;

    // Top-10 tracks (by title)
    let mut stmt = conn.prepare(
        "SELECT title, COUNT(*) as cnt FROM download_history
         GROUP BY title ORDER BY cnt DESC LIMIT 10",
    )?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;

    let mut top_tracks = Vec::new();
    for row in rows {
        if let Ok((title, count)) = row {
            top_tracks.push((title, count));
        }
    }

    // Top formats
    let mut stmt = conn.prepare(
        "SELECT format, COUNT(*) as cnt FROM download_history
         GROUP BY format ORDER BY cnt DESC",
    )?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;

    let mut top_formats = Vec::new();
    for row in rows {
        if let Ok((format, count)) = row {
            top_formats.push((format, count));
        }
    }

    Ok(GlobalStats {
        total_users,
        total_downloads,
        top_tracks,
        top_formats,
    })
}

/// Gets all download history for a user for export
pub fn get_all_download_history(conn: &DbConnection, telegram_id: i64) -> Result<Vec<DownloadHistoryEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, url, title, format, downloaded_at, file_id, author, file_size, duration, video_quality, audio_bitrate,
                bot_api_url, bot_api_is_local, source_id, part_index, category
         FROM download_history
         WHERE user_id = ? ORDER BY downloaded_at DESC",
    )?;
    let rows = stmt.query_map(rusqlite::params![telegram_id], |row| {
        Ok(DownloadHistoryEntry {
            id: row.get(0)?,
            url: row.get(1)?,
            title: row.get(2)?,
            format: row.get(3)?,
            downloaded_at: row.get(4)?,
            file_id: row.get(5)?,
            author: row.get(6)?,
            file_size: row.get(7)?,
            duration: row.get(8)?,
            video_quality: row.get(9)?,
            audio_bitrate: row.get(10)?,
            bot_api_url: row.get(11)?,
            bot_api_is_local: row.get(12)?,
            source_id: row.get(13)?,
            part_index: row.get(14)?,
            category: row.get(15)?,
        })
    })?;

    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }
    Ok(entries)
}

/// Gets filtered download history for the /downloads command
///
/// Returns only files with file_id (successfully sent) and only mp3/mp4 (excluding subtitles).
/// Supports filtering by file type and searching by title/author.
pub fn get_download_history_filtered(
    conn: &DbConnection,
    user_id: i64,
    file_type_filter: Option<&str>,
    search_text: Option<&str>,
    category_filter: Option<&str>,
) -> Result<Vec<DownloadHistoryEntry>> {
    let mut query = String::from(
        "SELECT id, url, title, format, downloaded_at, file_id, author, file_size,
         duration, video_quality, audio_bitrate, bot_api_url, bot_api_is_local, source_id, part_index, category
         FROM download_history WHERE user_id = ?",
    );

    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];

    // Only show files with file_id (successfully sent files)
    query.push_str(" AND file_id IS NOT NULL");

    // Only show mp3/mp4 (exclude subtitles)
    query.push_str(" AND (format = 'mp3' OR format = 'mp4')");

    if let Some(ft) = file_type_filter {
        query.push_str(" AND format = ?");
        params.push(Box::new(ft.to_string()));
    }

    if let Some(search) = search_text {
        query.push_str(" AND (title LIKE ? OR author LIKE ?)");
        let search_pattern = format!("%{}%", search);
        params.push(Box::new(search_pattern.clone()));
        params.push(Box::new(search_pattern));
    }

    if let Some(cat) = category_filter {
        query.push_str(" AND category = ?");
        params.push(Box::new(cat.to_string()));
    }

    query.push_str(" ORDER BY downloaded_at DESC");

    let mut stmt = conn.prepare(&query)?;
    let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let downloads = stmt
        .query_map(params_refs.as_slice(), |row| {
            Ok(DownloadHistoryEntry {
                id: row.get(0)?,
                url: row.get(1)?,
                title: row.get(2)?,
                format: row.get(3)?,
                downloaded_at: row.get(4)?,
                file_id: row.get(5)?,
                author: row.get(6)?,
                file_size: row.get(7)?,
                duration: row.get(8)?,
                video_quality: row.get(9)?,
                audio_bitrate: row.get(10)?,
                bot_api_url: row.get(11)?,
                bot_api_is_local: row.get(12)?,
                source_id: row.get(13)?,
                part_index: row.get(14)?,
                category: row.get(15)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(downloads)
}
