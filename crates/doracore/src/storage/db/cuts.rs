//! Audio/video clip (cut) history operations.

use super::DbConnection;
use rusqlite::Result;

#[derive(Debug, Clone)]
pub struct CutEntry {
    pub id: i64,
    pub user_id: i64,
    pub original_url: String,
    pub source_kind: String,
    pub source_id: i64,
    pub output_kind: String,
    pub segments_json: String,
    pub segments_text: String,
    pub title: String,
    pub created_at: String,
    pub file_id: Option<String>,
    pub file_size: Option<i64>,
    pub duration: Option<i64>,
    pub video_quality: Option<String>,
}

pub fn create_cut(
    conn: &DbConnection,
    user_id: i64,
    original_url: &str,
    source_kind: &str,
    source_id: i64,
    output_kind: &str,
    segments_json: &str,
    segments_text: &str,
    title: &str,
    file_id: Option<&str>,
    file_size: Option<i64>,
    duration: Option<i64>,
    video_quality: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO cuts (
            user_id, original_url, source_kind, source_id, output_kind, segments_json, segments_text,
            title, file_id, file_size, duration, video_quality
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        rusqlite::params![
            user_id,
            original_url,
            source_kind,
            source_id,
            output_kind,
            segments_json,
            segments_text,
            title,
            file_id,
            file_size,
            duration,
            video_quality,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_cuts_count(conn: &DbConnection, user_id: i64) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM cuts WHERE user_id = ?1",
        rusqlite::params![user_id],
        |row| row.get(0),
    )
}

pub fn get_cuts_page(conn: &DbConnection, user_id: i64, limit: i64, offset: i64) -> Result<Vec<CutEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, original_url, source_kind, source_id, output_kind, segments_json, segments_text,
                title, created_at, file_id, file_size, duration, video_quality
         FROM cuts
         WHERE user_id = ?1
         ORDER BY created_at DESC
         LIMIT ?2 OFFSET ?3",
    )?;
    let rows = stmt.query_map(rusqlite::params![user_id, limit, offset], |row| {
        Ok(CutEntry {
            id: row.get(0)?,
            user_id: row.get(1)?,
            original_url: row.get(2)?,
            source_kind: row.get(3)?,
            source_id: row.get(4)?,
            output_kind: row.get(5)?,
            segments_json: row.get(6)?,
            segments_text: row.get(7)?,
            title: row.get(8)?,
            created_at: row.get(9)?,
            file_id: row.get(10)?,
            file_size: row.get(11)?,
            duration: row.get(12)?,
            video_quality: row.get(13)?,
        })
    })?;

    rows.collect::<Result<Vec<_>>>()
}

pub fn get_cut_entry(conn: &DbConnection, user_id: i64, cut_id: i64) -> Result<Option<CutEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, original_url, source_kind, source_id, output_kind, segments_json, segments_text,
                title, created_at, file_id, file_size, duration, video_quality
         FROM cuts
         WHERE id = ?1 AND user_id = ?2",
    )?;
    let mut rows = stmt.query(rusqlite::params![cut_id, user_id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(CutEntry {
            id: row.get(0)?,
            user_id: row.get(1)?,
            original_url: row.get(2)?,
            source_kind: row.get(3)?,
            source_id: row.get(4)?,
            output_kind: row.get(5)?,
            segments_json: row.get(6)?,
            segments_text: row.get(7)?,
            title: row.get(8)?,
            created_at: row.get(9)?,
            file_id: row.get(10)?,
            file_size: row.get(11)?,
            duration: row.get(12)?,
            video_quality: row.get(13)?,
        }))
    } else {
        Ok(None)
    }
}
