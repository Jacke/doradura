//! User uploads storage module for premium/vip users
//!
//! Handles CRUD operations for uploaded media files (photos, videos, documents)
//! that users can later convert to various formats.

use super::db::DbConnection;
use rusqlite::Result;

/// Structure representing an uploaded file entry
#[derive(Debug, Clone)]
pub struct UploadEntry {
    /// Unique ID of the upload
    pub id: i64,
    /// Telegram ID of the user who uploaded the file
    pub user_id: i64,
    /// Original filename from Telegram
    pub original_filename: Option<String>,
    /// Display title (can be renamed by user)
    pub title: String,
    /// Media type: 'photo', 'video', 'document', 'audio'
    pub media_type: String,
    /// File format/extension: 'mp4', 'jpg', 'docx', etc.
    pub file_format: Option<String>,
    /// Telegram file_id for retrieval
    pub file_id: String,
    /// Telegram file_unique_id for deduplication
    pub file_unique_id: Option<String>,
    /// File size in bytes
    pub file_size: Option<i64>,
    /// Duration in seconds (for video/audio)
    pub duration: Option<i64>,
    /// Width in pixels (for photo/video)
    pub width: Option<i32>,
    /// Height in pixels (for photo/video)
    pub height: Option<i32>,
    /// MIME type
    pub mime_type: Option<String>,
    /// Message ID where file was sent (for MTProto fallback)
    pub message_id: Option<i32>,
    /// Chat ID where message was sent
    pub chat_id: Option<i64>,
    /// Upload timestamp
    pub uploaded_at: String,
    /// Thumbnail file_id (for video)
    pub thumbnail_file_id: Option<String>,
}

/// Parameters for saving a new upload
#[derive(Debug)]
pub struct NewUpload<'a> {
    pub user_id: i64,
    pub original_filename: Option<&'a str>,
    pub title: &'a str,
    pub media_type: &'a str,
    pub file_format: Option<&'a str>,
    pub file_id: &'a str,
    pub file_unique_id: Option<&'a str>,
    pub file_size: Option<i64>,
    pub duration: Option<i64>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub mime_type: Option<&'a str>,
    pub message_id: Option<i32>,
    pub chat_id: Option<i64>,
    pub thumbnail_file_id: Option<&'a str>,
}

/// Saves a new upload to the database
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `upload` - Upload parameters
///
/// # Returns
///
/// Returns `Ok(id)` on success (ID of the inserted row) or database error.
pub fn save_upload(conn: &DbConnection, upload: &NewUpload) -> Result<i64> {
    conn.execute(
        "INSERT INTO uploads (
            user_id, original_filename, title, media_type, file_format,
            file_id, file_unique_id, file_size, duration, width, height,
            mime_type, message_id, chat_id, thumbnail_file_id
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        rusqlite::params![
            upload.user_id,
            upload.original_filename,
            upload.title,
            upload.media_type,
            upload.file_format,
            upload.file_id,
            upload.file_unique_id,
            upload.file_size,
            upload.duration,
            upload.width,
            upload.height,
            upload.mime_type,
            upload.message_id,
            upload.chat_id,
            upload.thumbnail_file_id,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Gets an upload entry by ID for a specific user
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `user_id` - Telegram ID of the user
/// * `upload_id` - ID of the upload
///
/// # Returns
///
/// Returns `Ok(Some(UploadEntry))` if found, `Ok(None)` if not found.
pub fn get_upload_by_id(conn: &DbConnection, user_id: i64, upload_id: i64) -> Result<Option<UploadEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, original_filename, title, media_type, file_format,
                file_id, file_unique_id, file_size, duration, width, height,
                mime_type, message_id, chat_id, uploaded_at, thumbnail_file_id
         FROM uploads WHERE id = ? AND user_id = ?",
    )?;

    let mut rows = stmt.query(rusqlite::params![upload_id, user_id])?;

    if let Some(row) = rows.next()? {
        Ok(Some(UploadEntry {
            id: row.get(0)?,
            user_id: row.get(1)?,
            original_filename: row.get(2)?,
            title: row.get(3)?,
            media_type: row.get(4)?,
            file_format: row.get(5)?,
            file_id: row.get(6)?,
            file_unique_id: row.get(7)?,
            file_size: row.get(8)?,
            duration: row.get(9)?,
            width: row.get(10)?,
            height: row.get(11)?,
            mime_type: row.get(12)?,
            message_id: row.get(13)?,
            chat_id: row.get(14)?,
            uploaded_at: row.get(15)?,
            thumbnail_file_id: row.get(16)?,
        }))
    } else {
        Ok(None)
    }
}

/// Gets filtered uploads for the /videos command
///
/// Supports filtering by media type and text search in title.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `user_id` - Telegram ID of the user
/// * `media_type_filter` - Optional media type filter ('photo', 'video', 'document', 'audio')
/// * `search_text` - Optional text to search in title
///
/// # Returns
///
/// Returns a vector of UploadEntry sorted by upload date (newest first).
pub fn get_uploads_filtered(
    conn: &DbConnection,
    user_id: i64,
    media_type_filter: Option<&str>,
    search_text: Option<&str>,
) -> Result<Vec<UploadEntry>> {
    let mut query = String::from(
        "SELECT id, user_id, original_filename, title, media_type, file_format,
                file_id, file_unique_id, file_size, duration, width, height,
                mime_type, message_id, chat_id, uploaded_at, thumbnail_file_id
         FROM uploads WHERE user_id = ?",
    );

    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];

    if let Some(mt) = media_type_filter {
        query.push_str(" AND media_type = ?");
        params.push(Box::new(mt.to_string()));
    }

    if let Some(search) = search_text {
        query.push_str(" AND title LIKE ?");
        let search_pattern = format!("%{}%", search);
        params.push(Box::new(search_pattern));
    }

    query.push_str(" ORDER BY uploaded_at DESC");

    let mut stmt = conn.prepare(&query)?;
    let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let uploads = stmt
        .query_map(params_refs.as_slice(), |row| {
            Ok(UploadEntry {
                id: row.get(0)?,
                user_id: row.get(1)?,
                original_filename: row.get(2)?,
                title: row.get(3)?,
                media_type: row.get(4)?,
                file_format: row.get(5)?,
                file_id: row.get(6)?,
                file_unique_id: row.get(7)?,
                file_size: row.get(8)?,
                duration: row.get(9)?,
                width: row.get(10)?,
                height: row.get(11)?,
                mime_type: row.get(12)?,
                message_id: row.get(13)?,
                chat_id: row.get(14)?,
                uploaded_at: row.get(15)?,
                thumbnail_file_id: row.get(16)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(uploads)
}

/// Gets the total count of uploads for a user
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `user_id` - Telegram ID of the user
///
/// # Returns
///
/// Returns the count of uploads.
pub fn get_uploads_count(conn: &DbConnection, user_id: i64) -> Result<i64> {
    let mut stmt = conn.prepare("SELECT COUNT(*) FROM uploads WHERE user_id = ?")?;
    let count: i64 = stmt.query_row(rusqlite::params![user_id], |row| row.get(0))?;
    Ok(count)
}

/// Deletes an upload by ID
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `user_id` - Telegram ID of the user (for authorization check)
/// * `upload_id` - ID of the upload to delete
///
/// # Returns
///
/// Returns `Ok(true)` if the upload was deleted, `Ok(false)` if not found.
pub fn delete_upload(conn: &DbConnection, user_id: i64, upload_id: i64) -> Result<bool> {
    let rows_affected = conn.execute(
        "DELETE FROM uploads WHERE id = ? AND user_id = ?",
        rusqlite::params![upload_id, user_id],
    )?;
    Ok(rows_affected > 0)
}

/// Updates the title of an upload
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `user_id` - Telegram ID of the user (for authorization check)
/// * `upload_id` - ID of the upload
/// * `new_title` - New title for the upload
///
/// # Returns
///
/// Returns `Ok(true)` if the upload was updated, `Ok(false)` if not found.
pub fn update_upload_title(conn: &DbConnection, user_id: i64, upload_id: i64, new_title: &str) -> Result<bool> {
    let rows_affected = conn.execute(
        "UPDATE uploads SET title = ? WHERE id = ? AND user_id = ?",
        rusqlite::params![new_title, upload_id, user_id],
    )?;
    Ok(rows_affected > 0)
}

/// Checks if a file with the same file_unique_id already exists for the user
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `user_id` - Telegram ID of the user
/// * `file_unique_id` - Telegram file_unique_id to check
///
/// # Returns
///
/// Returns `Ok(Some(UploadEntry))` if duplicate found, `Ok(None)` otherwise.
pub fn find_duplicate_upload(conn: &DbConnection, user_id: i64, file_unique_id: &str) -> Result<Option<UploadEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, original_filename, title, media_type, file_format,
                file_id, file_unique_id, file_size, duration, width, height,
                mime_type, message_id, chat_id, uploaded_at, thumbnail_file_id
         FROM uploads WHERE user_id = ? AND file_unique_id = ?",
    )?;

    let mut rows = stmt.query(rusqlite::params![user_id, file_unique_id])?;

    if let Some(row) = rows.next()? {
        Ok(Some(UploadEntry {
            id: row.get(0)?,
            user_id: row.get(1)?,
            original_filename: row.get(2)?,
            title: row.get(3)?,
            media_type: row.get(4)?,
            file_format: row.get(5)?,
            file_id: row.get(6)?,
            file_unique_id: row.get(7)?,
            file_size: row.get(8)?,
            duration: row.get(9)?,
            width: row.get(10)?,
            height: row.get(11)?,
            mime_type: row.get(12)?,
            message_id: row.get(13)?,
            chat_id: row.get(14)?,
            uploaded_at: row.get(15)?,
            thumbnail_file_id: row.get(16)?,
        }))
    } else {
        Ok(None)
    }
}

/// Updates the message_id for an upload (for MTProto file_reference refresh)
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `upload_id` - ID of the upload
/// * `message_id` - New message ID
/// * `chat_id` - Chat ID where the message was sent
///
/// # Returns
///
/// Returns `Ok(())` on success.
pub fn update_upload_message_id(conn: &DbConnection, upload_id: i64, message_id: i32, chat_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE uploads SET message_id = ?, chat_id = ? WHERE id = ?",
        rusqlite::params![message_id, chat_id, upload_id],
    )?;
    Ok(())
}

// Tests are in the integration test suite to use proper DbPool with migrations
// See tests/uploads_test.rs
