//! Error logging, feedback, and lyrics session operations.

use super::DbConnection;
use rusqlite::Result;

/// Saves user feedback to the database.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `user_id` - Telegram ID of the user
/// * `username` - Username of the user (optional)
/// * `first_name` - First name of the user
/// * `message` - Feedback text
///
/// # Returns
///
/// Returns `Result<i64>` with the ID of the created record or an error.
pub fn save_feedback(
    conn: &DbConnection,
    user_id: i64,
    username: Option<&str>,
    first_name: &str,
    message: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO feedback_messages (user_id, username, first_name, message, status)
         VALUES (?1, ?2, ?3, ?4, 'new')",
        rusqlite::params![user_id, username, first_name, message],
    )?;

    Ok(conn.last_insert_rowid())
}

// ==================== Error Log ====================

// ==================== Error Log ====================

/// Error log entry
#[derive(Debug, Clone)]
pub struct ErrorLogEntry {
    pub id: i64,
    pub timestamp: String,
    pub user_id: Option<i64>,
    pub username: Option<String>,
    pub error_type: String,
    pub error_message: String,
    pub url: Option<String>,
    pub context: Option<String>,
    pub resolved: bool,
}

/// Logs an error to the database
pub fn log_error(
    conn: &DbConnection,
    user_id: Option<i64>,
    username: Option<&str>,
    error_type: &str,
    error_message: &str,
    url: Option<&str>,
    context: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO error_log (user_id, username, error_type, error_message, url, context)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![user_id, username, error_type, error_message, url, context],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Gets recent errors (last N hours)
pub fn get_recent_errors(conn: &DbConnection, hours: i64, limit: i64) -> Result<Vec<ErrorLogEntry>> {
    let since = chrono::Utc::now() - chrono::Duration::hours(hours);
    let since_str = since.format("%Y-%m-%d %H:%M:%S").to_string();

    let mut stmt = conn.prepare(
        "SELECT id, timestamp, user_id, username, error_type, error_message, url, context, resolved
         FROM error_log
         WHERE timestamp >= ?1
         ORDER BY timestamp DESC
         LIMIT ?2",
    )?;

    let rows = stmt.query_map(rusqlite::params![since_str, limit], |row| {
        Ok(ErrorLogEntry {
            id: row.get(0)?,
            timestamp: row.get(1)?,
            user_id: row.get(2)?,
            username: row.get(3)?,
            error_type: row.get(4)?,
            error_message: row.get(5)?,
            url: row.get(6)?,
            context: row.get(7)?,
            resolved: row.get::<_, i32>(8)? != 0,
        })
    })?;

    let mut errors = Vec::new();
    for row in rows.flatten() {
        errors.push(row);
    }
    Ok(errors)
}

/// Gets error count by type for a period
pub fn get_error_stats(conn: &DbConnection, hours: i64) -> Result<Vec<(String, i64)>> {
    let since = chrono::Utc::now() - chrono::Duration::hours(hours);
    let since_str = since.format("%Y-%m-%d %H:%M:%S").to_string();

    let mut stmt = conn.prepare(
        "SELECT error_type, COUNT(*) as cnt
         FROM error_log
         WHERE timestamp >= ?1
         GROUP BY error_type
         ORDER BY cnt DESC",
    )?;

    let rows = stmt.query_map([&since_str], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;

    let mut stats = Vec::new();
    for row in rows.flatten() {
        stats.push(row);
    }
    Ok(stats)
}

/// Cleans up old error logs (older than N days)
pub fn cleanup_old_errors(conn: &DbConnection, days: i64) -> Result<usize> {
    let cutoff = chrono::Utc::now() - chrono::Duration::days(days);
    let cutoff_str = cutoff.format("%Y-%m-%d %H:%M:%S").to_string();

    let deleted = conn.execute("DELETE FROM error_log WHERE timestamp < ?1", [&cutoff_str])?;
    Ok(deleted)
}

// ==================== Lyrics Sessions ====================

/// Store fetched lyrics (with parsed sections as JSON) for later retrieval by section.
pub fn create_lyrics_session(
    conn: &DbConnection,
    id: &str,
    user_id: i64,
    artist: &str,
    title: &str,
    sections_json: &str,
    has_structure: bool,
) -> Result<()> {
    let now = chrono::Utc::now();
    let expires_at = now + chrono::Duration::hours(24);
    conn.execute(
        "INSERT INTO lyrics_sessions (id, user_id, artist, title, sections_json, has_structure, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            id,
            user_id,
            artist,
            title,
            sections_json,
            has_structure as i32,
            now.to_rfc3339(),
            expires_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

/// Retrieve a lyrics session by ID. Returns (artist, title, sections_json, has_structure).
pub fn get_lyrics_session(conn: &DbConnection, id: &str) -> Result<Option<(String, String, String, bool)>> {
    let result = conn.query_row(
        "SELECT artist, title, sections_json, has_structure FROM lyrics_sessions WHERE id = ?1 AND expires_at > ?2",
        rusqlite::params![id, chrono::Utc::now().to_rfc3339()],
        |row| {
            let has_structure: i32 = row.get(3)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                has_structure != 0,
            ))
        },
    );
    match result {
        Ok(v) => Ok(Some(v)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}
