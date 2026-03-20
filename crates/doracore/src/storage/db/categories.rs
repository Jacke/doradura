//! User categories and category session operations.

use super::DbConnection;
use super::DownloadHistoryEntry;
use rusqlite::Result;

// ==================== User Categories ====================

/// Creates a user category (or ignores if it already exists).
pub fn create_user_category(conn: &DbConnection, user_id: i64, name: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO user_categories (user_id, name) VALUES (?1, ?2)",
        rusqlite::params![user_id, name],
    )?;
    Ok(())
}

/// Returns the user's category names ordered alphabetically.
pub fn get_user_categories(conn: &DbConnection, user_id: i64) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT name FROM user_categories WHERE user_id = ? ORDER BY name")?;
    let rows = stmt.query_map(rusqlite::params![user_id], |row| row.get(0))?;
    let mut cats = Vec::new();
    for row in rows {
        cats.push(row?);
    }
    Ok(cats)
}

/// Sets (or clears) the category on a download history entry.
pub fn set_download_category(
    conn: &DbConnection,
    user_id: i64,
    download_id: i64,
    category: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE download_history SET category = ?1 WHERE id = ?2 AND user_id = ?3",
        rusqlite::params![category, download_id, user_id],
    )?;
    Ok(())
}

// ==================== New Category Sessions ====================

/// Stores a new-category session: user is creating a category for a specific download.
pub fn create_new_category_session(conn: &DbConnection, user_id: i64, download_id: i64) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO new_category_sessions (user_id, download_id, created_at) VALUES (?1, ?2, datetime('now'))",
        rusqlite::params![user_id, download_id],
    )?;
    Ok(())
}

/// Returns the download_id for an active new-category session, or None.
pub fn get_active_new_category_session(conn: &DbConnection, user_id: i64) -> Result<Option<i64>> {
    let mut stmt = conn.prepare(
        "SELECT download_id FROM new_category_sessions WHERE user_id = ? AND created_at > datetime('now', '-10 minutes')",
    )?;
    let mut rows = stmt.query_map(rusqlite::params![user_id], |row| row.get(0))?;
    if let Some(row) = rows.next() {
        Ok(Some(row?))
    } else {
        Ok(None)
    }
}

/// Deletes the new-category session for a user.
pub fn delete_new_category_session(conn: &DbConnection, user_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM new_category_sessions WHERE user_id = ?",
        rusqlite::params![user_id],
    )?;
    Ok(())
}

/// Gets filtered cuts history for the /downloads command
pub fn get_cuts_history_filtered(
    conn: &DbConnection,
    user_id: i64,
    search_text: Option<&str>,
) -> Result<Vec<DownloadHistoryEntry>> {
    let mut query = String::from(
        "SELECT id, original_url, title, output_kind, created_at, file_id, file_size,
         duration, video_quality FROM cuts WHERE user_id = ?",
    );

    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];

    // Only show files with file_id
    query.push_str(" AND file_id IS NOT NULL");

    if let Some(search) = search_text {
        query.push_str(" AND title LIKE ?");
        let search_pattern = format!("%{}%", search);
        params.push(Box::new(search_pattern));
    }

    query.push_str(" ORDER BY created_at DESC");

    let mut stmt = conn.prepare(&query)?;
    let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let cuts = stmt
        .query_map(params_refs.as_slice(), |row| {
            Ok(DownloadHistoryEntry {
                id: row.get(0)?,
                url: row.get(1)?,
                title: row.get(2)?,
                format: String::from("edit"), // Marker for UI
                downloaded_at: row.get(4)?,
                file_id: row.get(5)?,
                author: None,
                file_size: row.get(6)?,
                duration: row.get(7)?,
                video_quality: row.get(8)?,
                audio_bitrate: None,
                bot_api_url: None,
                bot_api_is_local: None,
                source_id: None,
                part_index: None,
                category: None,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(cuts)
}
