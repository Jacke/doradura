//! Database access layer -- re-exports from sub-modules.

mod categories;
mod cuts;
mod download_history;
mod errors;
mod playlists;
mod pool;
mod sessions;
mod subscriptions;
mod synced_playlists;
mod task_queue;
mod users;
mod vault;
pub use categories::*;
pub use cuts::*;
pub use download_history::*;
pub use errors::*;
pub use playlists::*;
pub use pool::*;
pub use sessions::*;
pub use subscriptions::*;
pub use synced_playlists::*;
pub use task_queue::*;
pub use users::*;
pub use vault::*;

use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Result;

/// Connection timeout for pool.get() calls - prevents indefinite blocking.
/// 3s gives enough room for SQLite busy_timeout (5s PRAGMA) while still failing fast
/// if the pool is genuinely exhausted.
const CONNECTION_TIMEOUT_SECS: u64 = 3;

pub type DbPool = Pool<SqliteConnectionManager>;
pub type DbConnection = PooledConnection<SqliteConnectionManager>;

// ==================== Bot Assets ====================

/// Get a cached bot asset file_id by key (e.g. "ringtone_instruction_iphone_1")
pub fn get_bot_asset(conn: &DbConnection, key: &str) -> Result<Option<String>> {
    let result = conn.query_row("SELECT file_id FROM bot_assets WHERE key = ?1", [key], |row| row.get(0));
    match result {
        Ok(file_id) => Ok(Some(file_id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Set (upsert) a bot asset file_id for a key
pub fn set_bot_asset(conn: &DbConnection, key: &str, file_id: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO bot_assets (key, file_id, created_at) VALUES (?1, ?2, datetime('now'))",
        rusqlite::params![key, file_id],
    )?;
    Ok(())
}

// ==================== Video Timestamps ====================

use crate::timestamps::{TimestampSource, VideoTimestamp};

/// Save timestamps extracted from a video for later use in clip suggestions
pub fn save_video_timestamps(conn: &DbConnection, download_id: i64, timestamps: &[VideoTimestamp]) -> Result<()> {
    for ts in timestamps {
        conn.execute(
            "INSERT INTO video_timestamps (download_id, source, time_seconds, end_seconds, label)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                download_id,
                ts.source.as_str(),
                ts.time_seconds,
                ts.end_seconds,
                ts.label,
            ],
        )?;
    }
    Ok(())
}

/// Get timestamps for a download entry
pub fn get_video_timestamps(conn: &DbConnection, download_id: i64) -> Result<Vec<VideoTimestamp>> {
    let mut stmt = conn.prepare(
        "SELECT source, time_seconds, end_seconds, label
         FROM video_timestamps
         WHERE download_id = ?1
         ORDER BY time_seconds ASC",
    )?;

    let rows = stmt.query_map([download_id], |row| {
        let source_str: String = row.get(0)?;
        Ok(VideoTimestamp {
            source: TimestampSource::parse(&source_str),
            time_seconds: row.get(1)?,
            end_seconds: row.get(2)?,
            label: row.get(3)?,
        })
    })?;

    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::Plan;
    use rusqlite::Connection;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tempfile::NamedTempFile;

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Helper function to create a test database with schema
    fn setup_test_db() -> DbPool {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let db_path = std::env::temp_dir().join(format!("doradura_test_{}_{}.db", std::process::id(), counter));

        // Remove existing file if any
        let _ = std::fs::remove_file(&db_path);

        let db_path_str = db_path.to_string_lossy().to_string();

        // Explicitly open and run migrations (use test-specific function without outer transaction)
        {
            let mut conn = Connection::open(&db_path_str).expect("Failed to open database");
            crate::storage::migrations::run_migrations_for_test(&mut conn).expect("Failed to run migrations");
        }

        // Now create the pool
        let manager = r2d2_sqlite::SqliteConnectionManager::file(&db_path_str);
        r2d2::Pool::builder()
            .max_size(5)
            .build(manager)
            .expect("Failed to create test database pool")
    }

    /// Helper to create a test database with an in-memory connection
    #[allow(dead_code)]
    fn setup_memory_db() -> Connection {
        let mut conn = Connection::open(":memory:").unwrap();
        crate::storage::migrations::run_migrations_for_test(&mut conn).unwrap();
        conn
    }

    // ==================== User CRUD Tests ====================

    #[test]
    fn test_create_user() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        let result = create_user(&conn, 12345, Some("testuser".to_string()));
        assert!(result.is_ok());

        // Verify user was created
        let user = get_user(&conn, 12345).unwrap();
        assert!(user.is_some());
        let user = user.unwrap();
        assert_eq!(user.telegram_id, 12345);
        assert_eq!(user.username, Some("testuser".to_string()));
        assert_eq!(user.plan, Plan::Free);
        assert_eq!(user.download_format, "mp3");
        assert_eq!(user.language, "en");
    }

    #[test]
    fn test_create_user_with_language() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        let result = create_user_with_language(&conn, 12346, Some("ruuser".to_string()), "ru");
        assert!(result.is_ok());

        let user = get_user(&conn, 12346).unwrap().unwrap();
        assert_eq!(user.language, "ru");
    }

    #[test]
    fn test_create_user_without_username() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        let result = create_user(&conn, 12347, None);
        assert!(result.is_ok());

        let user = get_user(&conn, 12347).unwrap().unwrap();
        assert_eq!(user.username, None);
    }

    #[test]
    fn test_get_nonexistent_user() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        let user = get_user(&conn, 99999).unwrap();
        assert!(user.is_none());
    }

    #[test]
    fn test_user_struct_methods() {
        let user = User {
            telegram_id: 123,
            username: Some("test".to_string()),
            plan: crate::core::types::Plan::Premium,
            download_format: "mp4".to_string(),
            download_subtitles: 1,
            video_quality: "1080p".to_string(),
            audio_bitrate: "320k".to_string(),
            send_as_document: 0,
            send_audio_as_document: 1,
            subscription_expires_at: None,
            telegram_charge_id: None,
            language: "en".to_string(),
            is_recurring: false,
            burn_subtitles: 0,
            progress_bar_style: "classic".to_string(),
            is_blocked: false,
            experimental_features: 0,
        };

        assert_eq!(user.telegram_id(), 123);
        assert_eq!(user.download_format(), "mp4");
    }

    // ==================== User Settings Tests ====================

    #[test]
    fn test_download_format_settings() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12350, None).unwrap();

        // Default format
        let format = get_user_download_format(&conn, 12350).unwrap();
        assert_eq!(format, "mp3");

        // Change format
        set_user_download_format(&conn, 12350, "mp4").unwrap();
        let format = get_user_download_format(&conn, 12350).unwrap();
        assert_eq!(format, "mp4");
    }

    #[test]
    fn test_download_format_nonexistent_user() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        // Should return default "mp3" for nonexistent user
        let format = get_user_download_format(&conn, 99999).unwrap();
        assert_eq!(format, "mp3");
    }

    #[test]
    fn test_subtitles_settings() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12351, None).unwrap();

        // Default is disabled
        let enabled = get_user_download_subtitles(&conn, 12351).unwrap();
        assert!(!enabled);
    }

    #[test]
    fn test_burn_subtitles_settings() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12352, None).unwrap();

        // Default is disabled
        let enabled = get_user_burn_subtitles(&conn, 12352).unwrap();
        assert!(!enabled);

        // Enable burn subtitles
        set_user_burn_subtitles(&conn, 12352, true).unwrap();
        let enabled = get_user_burn_subtitles(&conn, 12352).unwrap();
        assert!(enabled);
    }

    #[test]
    fn test_subtitle_style_force_style() {
        // Default style: no MarginV, no Bold
        let default = SubtitleStyle::default();
        let style_str = default.to_force_style();
        assert!(style_str.contains("FontSize=24"));
        assert!(style_str.contains("Outline=2"));
        assert!(style_str.contains("Shadow=1"));
        assert!(!style_str.contains("MarginV"));
        assert!(!style_str.contains("Bold"));

        // Circle style: has MarginV and Bold
        let circle = SubtitleStyle::circle_default();
        let style_str = circle.to_force_style();
        assert!(style_str.contains("FontSize=16"));
        assert!(style_str.contains("Outline=2"));
        assert!(style_str.contains("Shadow=0"));
        assert!(style_str.contains("MarginV=55"));
        assert!(style_str.contains("Bold=1"));
    }

    #[test]
    fn test_video_quality_settings() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12353, None).unwrap();

        // Default quality
        let quality = get_user_video_quality(&conn, 12353).unwrap();
        assert_eq!(quality, "best");

        // Change quality
        set_user_video_quality(&conn, 12353, "720p").unwrap();
        let quality = get_user_video_quality(&conn, 12353).unwrap();
        assert_eq!(quality, "720p");
    }

    #[test]
    fn test_audio_bitrate_settings() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12354, None).unwrap();

        // Default bitrate
        let bitrate = get_user_audio_bitrate(&conn, 12354).unwrap();
        assert_eq!(bitrate, "320k");

        // Change bitrate
        set_user_audio_bitrate(&conn, 12354, "192k").unwrap();
        let bitrate = get_user_audio_bitrate(&conn, 12354).unwrap();
        assert_eq!(bitrate, "192k");
    }

    #[test]
    fn test_send_as_document_settings() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12355, None).unwrap();

        // Default is 0 (Media)
        let value = get_user_send_as_document(&conn, 12355).unwrap();
        assert_eq!(value, 0);

        // Change to Document
        set_user_send_as_document(&conn, 12355, 1).unwrap();
        let value = get_user_send_as_document(&conn, 12355).unwrap();
        assert_eq!(value, 1);
    }

    #[test]
    fn test_send_audio_as_document_settings() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12356, None).unwrap();

        // Default is 0 (Media)
        let value = get_user_send_audio_as_document(&conn, 12356).unwrap();
        assert_eq!(value, 0);

        // Change to Document
        set_user_send_audio_as_document(&conn, 12356, 1).unwrap();
        let value = get_user_send_audio_as_document(&conn, 12356).unwrap();
        assert_eq!(value, 1);
    }

    #[test]
    fn test_language_settings() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12357, None).unwrap();

        // Change language
        set_user_language(&conn, 12357, "fr").unwrap();
        let lang = get_user_language(&conn, 12357).unwrap();
        assert_eq!(lang, "fr");
    }

    // ==================== Plan/Subscription Tests ====================

    #[test]
    fn test_update_user_plan() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12360, None).unwrap();

        // Update to premium
        update_user_plan(&conn, 12360, "premium").unwrap();
        let user = get_user(&conn, 12360).unwrap().unwrap();
        assert_eq!(user.plan, Plan::Premium);
    }

    #[test]
    fn test_update_user_plan_to_free_clears_subscription_metadata() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12364, None).unwrap();
        update_subscription_data(&conn, 12364, "vip", "charge_123", "2026-12-31 00:00:00", true).unwrap();

        update_user_plan(&conn, 12364, "free").unwrap();

        let user = get_user(&conn, 12364).unwrap().unwrap();
        assert_eq!(user.plan, Plan::Free);
        assert_eq!(user.subscription_expires_at, None);
        assert_eq!(user.telegram_charge_id, None);
        assert!(!user.is_recurring);
    }

    #[test]
    fn test_update_user_plan_with_expiry() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12361, None).unwrap();

        // Update with 30-day expiry
        update_user_plan_with_expiry(&conn, 12361, "vip", Some(30)).unwrap();
        let user = get_user(&conn, 12361).unwrap().unwrap();
        assert_eq!(user.plan, Plan::Vip);
        assert!(user.subscription_expires_at.is_some());
    }

    #[test]
    fn test_update_user_plan_without_expiry() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12362, None).unwrap();

        // Update without expiry (free plan)
        update_user_plan_with_expiry(&conn, 12362, "free", None).unwrap();
        let user = get_user(&conn, 12362).unwrap().unwrap();
        assert_eq!(user.plan, Plan::Free);
    }

    #[test]
    fn test_set_user_blocked_roundtrip() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12365, Some("blocked_user".to_string())).unwrap();
        assert!(!is_user_blocked(&conn, 12365).unwrap());

        set_user_blocked(&conn, 12365, true).unwrap();
        assert!(is_user_blocked(&conn, 12365).unwrap());

        let user = get_user(&conn, 12365).unwrap().unwrap();
        assert!(user.is_blocked);

        set_user_blocked(&conn, 12365, false).unwrap();
        assert!(!is_user_blocked(&conn, 12365).unwrap());
    }

    #[test]
    fn test_search_users_includes_blocked_flag() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12366, Some("searchable_admin_target".to_string())).unwrap();
        set_user_blocked(&conn, 12366, true).unwrap();

        let users = search_users(&conn, "searchable_admin").unwrap();
        let user = users
            .into_iter()
            .find(|user| user.telegram_id == 12366)
            .expect("user should be returned by search");

        assert!(user.is_blocked);
    }

    #[test]
    fn test_is_premium_or_vip() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12363, None).unwrap();

        // Free user is not premium
        let result = is_premium_or_vip(&conn, 12363).unwrap();
        assert!(!result);

        // Update to premium
        update_user_plan(&conn, 12363, "premium").unwrap();
        let result = is_premium_or_vip(&conn, 12363).unwrap();
        assert!(result);

        // Update to vip
        update_user_plan(&conn, 12363, "vip").unwrap();
        let result = is_premium_or_vip(&conn, 12363).unwrap();
        assert!(result);
    }

    #[test]
    fn test_is_premium_or_vip_nonexistent_user() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        let result = is_premium_or_vip(&conn, 99999).unwrap();
        assert!(!result);
    }

    // ==================== Download History Tests ====================

    #[test]
    fn test_save_and_get_download_history() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12370, None).unwrap();

        // Save download
        let id = save_download_history(
            &conn,
            12370,
            "https://youtube.com/watch?v=test",
            "Test Song",
            "mp3",
            Some("file123"),
            Some("Test Artist"),
            Some(5000000),
            Some(180),
            None,
            Some("320k"),
            None,
            None,
        )
        .unwrap();

        assert!(id > 0);

        // Get history
        let history = get_download_history(&conn, 12370, Some(10)).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].title, "Test Song");
        assert_eq!(history[0].format, "mp3");
        assert_eq!(history[0].file_id, Some("file123".to_string()));
    }

    #[test]
    fn test_download_history_with_parts() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12371, None).unwrap();

        // Save main download
        let main_id = save_download_history(
            &conn,
            12371,
            "https://example.com/video",
            "Long Video",
            "mp4",
            Some("main_file"),
            None,
            Some(500000000),
            Some(3600),
            Some("1080p"),
            None,
            None,
            None,
        )
        .unwrap();

        // Save part
        let _part_id = save_download_history(
            &conn,
            12371,
            "https://example.com/video",
            "Long Video (Part 1)",
            "mp4",
            Some("part1_file"),
            None,
            Some(100000000),
            Some(720),
            Some("1080p"),
            None,
            Some(main_id),
            Some(1),
        )
        .unwrap();

        let history = get_download_history(&conn, 12371, Some(10)).unwrap();
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn test_get_download_history_entry() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12372, None).unwrap();

        let id = save_download_history(
            &conn,
            12372,
            "https://example.com",
            "Test",
            "mp3",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let entry = get_download_history_entry(&conn, 12372, id).unwrap();
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().title, "Test");

        // Wrong user
        let entry = get_download_history_entry(&conn, 99999, id).unwrap();
        assert!(entry.is_none());
    }

    #[test]
    fn test_delete_download_history_entry() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12373, None).unwrap();

        let id = save_download_history(
            &conn,
            12373,
            "https://example.com",
            "Test",
            "mp3",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // Delete entry
        let deleted = delete_download_history_entry(&conn, 12373, id).unwrap();
        assert!(deleted);

        // Try to delete again (should fail)
        let deleted = delete_download_history_entry(&conn, 12373, id).unwrap();
        assert!(!deleted);
    }

    #[test]
    fn test_get_all_download_history() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12374, None).unwrap();

        for i in 0..5 {
            save_download_history(
                &conn,
                12374,
                &format!("https://example.com/{}", i),
                &format!("Test {}", i),
                "mp3",
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();
        }

        let all = get_all_download_history(&conn, 12374).unwrap();
        assert_eq!(all.len(), 5);
    }

    #[test]
    fn test_get_download_history_filtered() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12375, None).unwrap();

        // mp3 with file_id
        save_download_history(
            &conn,
            12375,
            "https://example.com/1",
            "Song 1",
            "mp3",
            Some("file1"),
            Some("Artist A"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // mp4 with file_id
        save_download_history(
            &conn,
            12375,
            "https://example.com/2",
            "Video 1",
            "mp4",
            Some("file2"),
            None,
            None,
            None,
            Some("720p"),
            None,
            None,
            None,
        )
        .unwrap();

        // mp3 without file_id (should be excluded)
        save_download_history(
            &conn,
            12375,
            "https://example.com/3",
            "Song 2",
            "mp3",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // srt (should be excluded)
        save_download_history(
            &conn,
            12375,
            "https://example.com/4",
            "Subtitles",
            "srt",
            Some("file4"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // No filter - should get mp3 and mp4 with file_id
        let filtered = get_download_history_filtered(&conn, 12375, None, None, None).unwrap();
        assert_eq!(filtered.len(), 2);

        // Filter by mp3
        let filtered = get_download_history_filtered(&conn, 12375, Some("mp3"), None, None).unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].format, "mp3");

        // Search by title
        let filtered = get_download_history_filtered(&conn, 12375, None, Some("Song"), None).unwrap();
        assert_eq!(filtered.len(), 1);

        // Search by author
        let filtered = get_download_history_filtered(&conn, 12375, None, Some("Artist A"), None).unwrap();
        assert_eq!(filtered.len(), 1);
    }

    // ==================== Task Queue Tests ====================

    #[test]
    fn test_task_queue_operations() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12380, None).unwrap();

        // Save task
        save_task_to_queue(
            &conn,
            "task-001",
            12380,
            "https://example.com",
            None,
            "mp3",
            false,
            None,
            Some("320k"),
            None,
            None,
            None,
            0,
            "12380:https://example.com:mp3:-:320k:audio",
        )
        .unwrap();

        // Get task
        let task = get_task_by_id(&conn, "task-001").unwrap();
        assert!(task.is_some());
        let task = task.unwrap();
        assert_eq!(task.status, "pending");
        assert_eq!(task.url, "https://example.com");
        assert!(!task.is_video);

        // Mark processing
        conn.execute(
            "UPDATE task_queue SET worker_id = 'worker-1', status = 'leased' WHERE id = 'task-001'",
            [],
        )
        .unwrap();
        mark_task_processing(&conn, "task-001", "worker-1").unwrap();
        let task = get_task_by_id(&conn, "task-001").unwrap().unwrap();
        assert_eq!(task.status, "processing");

        // Mark completed
        mark_task_completed(&conn, "task-001", "worker-1").unwrap();
        let task = get_task_by_id(&conn, "task-001").unwrap().unwrap();
        assert_eq!(task.status, "completed");
    }

    #[test]
    fn test_task_queue_failure() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12381, None).unwrap();

        save_task_to_queue(
            &conn,
            "task-002",
            12381,
            "https://example.com",
            None,
            "mp4",
            true,
            Some("720p"),
            None,
            None,
            None,
            None,
            1,
            "12381:https://example.com:mp4:720p:-:video",
        )
        .unwrap();

        // Mark failed
        conn.execute(
            "UPDATE task_queue SET worker_id = 'worker-2', status = 'processing' WHERE id = 'task-002'",
            [],
        )
        .unwrap();
        let will_retry = mark_task_failed(&conn, "task-002", "worker-2", "Download error", true, 5).unwrap();
        let task = get_task_by_id(&conn, "task-002").unwrap().unwrap();
        assert!(will_retry);
        assert_eq!(task.status, "pending");
        assert_eq!(task.error_message, Some("Download error".to_string()));
        assert_eq!(task.retry_count, 1);
    }

    #[test]
    fn test_update_task_status() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12382, None).unwrap();

        save_task_to_queue(
            &conn,
            "task-003",
            12382,
            "https://example.com",
            None,
            "mp3",
            false,
            None,
            None,
            None,
            None,
            None,
            0,
            "12382:https://example.com:mp3:-:-:audio",
        )
        .unwrap();

        update_task_status(&conn, "task-003", "custom_status", Some("Custom error")).unwrap();
        let task = get_task_by_id(&conn, "task-003").unwrap().unwrap();
        assert_eq!(task.status, "custom_status");
        assert_eq!(task.error_message, Some("Custom error".to_string()));
    }

    #[test]
    fn test_claim_next_task_uses_db_queue() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12383, None).unwrap();
        save_task_to_queue(
            &conn,
            "task-004",
            12383,
            "https://example.com/a",
            Some(99),
            "mp4",
            true,
            Some("720p"),
            None,
            Some("00:10"),
            Some("00:20"),
            Some(3),
            2,
            "12383:https://example.com/a:mp4:720p:-:video",
        )
        .unwrap();

        let claimed = claim_next_task(&conn, "worker-claim", 60).unwrap().unwrap();
        assert_eq!(claimed.status, "leased");
        assert_eq!(claimed.worker_id.as_deref(), Some("worker-claim"));
        assert_eq!(claimed.message_id, Some(99));
        assert_eq!(claimed.time_range_start.as_deref(), Some("00:10"));
        assert_eq!(claimed.time_range_end.as_deref(), Some("00:20"));
        assert_eq!(claimed.carousel_mask, Some(3));
    }

    #[test]
    fn test_register_processed_update_deduplicates() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        assert!(register_processed_update(&conn, 42, 1001).unwrap());
        assert!(!register_processed_update(&conn, 42, 1001).unwrap());
        assert!(register_processed_update(&conn, 43, 1001).unwrap());
    }

    // ==================== User Statistics Tests ====================

    #[test]
    fn test_get_user_stats() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12390, None).unwrap();

        // Add some downloads
        for i in 0..3 {
            save_download_history(
                &conn,
                12390,
                &format!("https://example.com/{}", i),
                &format!("Artist {} - Song {}", i % 2, i),
                "mp3",
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();
        }

        let stats = get_user_stats(&conn, 12390).unwrap();
        assert_eq!(stats.total_downloads, 3);
        assert!(stats.total_size > 0);
    }

    #[test]
    fn test_get_global_stats() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12391, None).unwrap();
        create_user(&conn, 12392, None).unwrap();

        save_download_history(
            &conn,
            12391,
            "https://example.com/1",
            "Song 1",
            "mp3",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        save_download_history(
            &conn,
            12392,
            "https://example.com/2",
            "Song 1",
            "mp3",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let stats = get_global_stats(&conn).unwrap();
        assert_eq!(stats.total_users, 2);
        assert_eq!(stats.total_downloads, 2);
        assert!(!stats.top_tracks.is_empty());
    }

    // ==================== Subscription Tests ====================

    #[test]
    fn test_subscription_operations() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12400, None).unwrap();

        // Get default subscription
        let sub = get_subscription(&conn, 12400).unwrap();
        assert!(sub.is_some());
        let sub = sub.unwrap();
        assert_eq!(sub.plan, Plan::Free);

        // Update subscription
        update_subscription_data(&conn, 12400, "premium", "charge_123", "2099-12-31T23:59:59Z", true).unwrap();

        let sub = get_subscription(&conn, 12400).unwrap().unwrap();
        assert_eq!(sub.plan, Plan::Premium);
        assert_eq!(sub.telegram_charge_id, Some("charge_123".to_string()));
        assert!(sub.is_recurring);

        // Check if active
        let active = is_subscription_active(&conn, 12400).unwrap();
        assert!(active);
    }

    #[test]
    fn test_cancel_subscription() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12401, None).unwrap();

        update_subscription_data(&conn, 12401, "premium", "charge_456", "2099-12-31T23:59:59Z", true).unwrap();

        cancel_subscription(&conn, 12401).unwrap();

        // cancel_subscription disables auto-renewal
        let sub = get_subscription(&conn, 12401).unwrap().unwrap();
        assert!(!sub.is_recurring, "is_recurring should be false after cancel");

        // The subscription plan in subscriptions table remains unchanged
        // (only is_recurring is updated in ON CONFLICT clause)
        // get_user reads from COALESCE(s.plan, u.plan) - subscriptions table takes precedence
        // This is the actual behavior - user keeps premium until expiry
    }

    #[test]
    fn test_cancel_subscription_for_new_user() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        // User without existing subscription
        create_user(&conn, 12411, None).unwrap();

        cancel_subscription(&conn, 12411).unwrap();

        // For new users (INSERT path), plan is set to 'free'
        let sub = get_subscription(&conn, 12411).unwrap().unwrap();
        assert_eq!(sub.plan, Plan::Free);
        assert!(!sub.is_recurring);
    }

    // ==================== Charge Tests ====================

    #[test]
    fn test_save_and_get_charges() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12410, None).unwrap();

        let id = save_charge(
            &conn,
            12410,
            "premium",
            "tg_charge_001",
            Some("provider_001"),
            "XTR",
            100,
            "premium_monthly",
            true,
            true,
            Some("2099-12-31T23:59:59Z"),
        )
        .unwrap();

        assert!(id > 0);

        let charges = get_user_charges(&conn, 12410).unwrap();
        assert_eq!(charges.len(), 1);
        assert_eq!(charges[0].plan, Plan::Premium);
        assert_eq!(charges[0].total_amount, 100);
        assert!(charges[0].is_recurring);
    }

    #[test]
    fn test_get_all_charges() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12411, None).unwrap();

        save_charge(
            &conn,
            12411,
            "premium",
            "tg_charge_002",
            None,
            "XTR",
            100,
            "premium",
            false,
            false,
            None,
        )
        .unwrap();

        save_charge(
            &conn,
            12411,
            "vip",
            "tg_charge_003",
            None,
            "XTR",
            200,
            "vip",
            false,
            false,
            None,
        )
        .unwrap();

        // Get all
        let all = get_all_charges(&conn, None, None, 0).unwrap();
        assert_eq!(all.len(), 2);

        // Filter by plan
        let premium_only = get_all_charges(&conn, Some("premium"), None, 0).unwrap();
        assert_eq!(premium_only.len(), 1);
    }

    #[test]
    fn test_get_charges_stats() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12412, None).unwrap();

        save_charge(&conn, 12412, "premium", "c1", None, "XTR", 100, "p", true, false, None).unwrap();
        save_charge(&conn, 12412, "vip", "c2", None, "XTR", 200, "v", false, false, None).unwrap();

        let (total, amount, premium, vip, recurring) = get_charges_stats(&conn).unwrap();
        assert_eq!(total, 2);
        assert_eq!(amount, 300);
        assert_eq!(premium, 1);
        assert_eq!(vip, 1);
        assert_eq!(recurring, 1);
    }

    // ==================== Video Clip Session Tests ====================

    #[test]
    fn test_video_clip_session_operations() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12430, None).unwrap();

        let session = VideoClipSession {
            id: "vcs-001".to_string(),
            user_id: 12430,
            source_download_id: 1,
            source_kind: SourceKind::Download,
            source_id: 1,
            original_url: "https://example.com".to_string(),
            output_kind: OutputKind::Cut,
            created_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            subtitle_lang: None,
        };

        upsert_video_clip_session(&conn, &session).unwrap();

        let active = get_active_video_clip_session(&conn, 12430).unwrap();
        assert!(active.is_some());
        assert_eq!(active.unwrap().id, "vcs-001");

        delete_video_clip_session_by_user(&conn, 12430).unwrap();
        let active = get_active_video_clip_session(&conn, 12430).unwrap();
        assert!(active.is_none());
    }

    // ==================== Cut Tests ====================

    #[test]
    fn test_cut_operations() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12440, None).unwrap();

        let id = create_cut(
            &conn,
            12440,
            "https://example.com",
            "download",
            1,
            "cut",
            "[{\"start\": 0, \"end\": 10}]",
            "0:00 - 0:10",
            "My Cut",
            Some("file_cut_1"),
            Some(1000000),
            Some(10),
            Some("720p"),
        )
        .unwrap();

        assert!(id > 0);

        let entry = get_cut_entry(&conn, 12440, id).unwrap();
        assert!(entry.is_some());

        let count = get_cuts_count(&conn, 12440).unwrap();
        assert_eq!(count, 1);

        let page = get_cuts_page(&conn, 12440, 10, 0).unwrap();
        assert_eq!(page.len(), 1);
    }

    // ==================== Cookies Upload Session Tests ====================

    #[test]
    fn test_cookies_upload_session_operations() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12450, None).unwrap();

        let session = CookiesUploadSession {
            id: "cookie-001".to_string(),
            user_id: 12450,
            created_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
        };

        upsert_cookies_upload_session(&conn, &session).unwrap();

        let active = get_active_cookies_upload_session(&conn, 12450).unwrap();
        assert!(active.is_some());
        assert_eq!(active.unwrap().id, "cookie-001");

        delete_cookies_upload_session_by_user(&conn, 12450).unwrap();
        let active = get_active_cookies_upload_session(&conn, 12450).unwrap();
        assert!(active.is_none());
    }

    // ==================== Audio Cut Session Tests ====================

    #[test]
    fn test_audio_cut_session_operations() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12460, None).unwrap();

        let session = AudioCutSession {
            id: "acs-001".to_string(),
            user_id: 12460,
            audio_session_id: "audio-001".to_string(),
            created_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
        };

        upsert_audio_cut_session(&conn, &session).unwrap();

        let active = get_active_audio_cut_session(&conn, 12460).unwrap();
        assert!(active.is_some());
        assert_eq!(active.unwrap().id, "acs-001");

        delete_audio_cut_session_by_user(&conn, 12460).unwrap();
        let active = get_active_audio_cut_session(&conn, 12460).unwrap();
        assert!(active.is_none());
    }

    // ==================== Request History Tests ====================

    #[test]
    #[ignore = "request_history table not in migrations"]
    fn test_log_request() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12470, None).unwrap();

        let result = log_request(&conn, 12470, "https://youtube.com/watch?v=test");
        assert!(result.is_ok());
    }

    // ==================== Message ID Update Tests ====================

    #[test]
    fn test_update_download_message_id() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12480, None).unwrap();

        let id = save_download_history(
            &conn,
            12480,
            "https://example.com",
            "Test",
            "mp3",
            Some("file_id"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        update_download_message_id(&conn, id, 123, 12480).unwrap();

        let info = get_download_message_info(&conn, id).unwrap();
        assert!(info.is_some());
        let (msg_id, chat_id) = info.unwrap();
        assert_eq!(msg_id, 123);
        assert_eq!(chat_id, 12480);
    }

    // ==================== All Users Test ====================

    #[test]
    fn test_get_all_users() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12490, Some("user1".to_string())).unwrap();
        create_user(&conn, 12491, Some("user2".to_string())).unwrap();

        let users = get_all_users(&conn).unwrap();
        assert!(users.len() >= 2);
    }

    // ==================== Sent Files Test ====================

    #[test]
    fn test_get_sent_files() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12500, Some("sender".to_string())).unwrap();

        save_download_history(
            &conn,
            12500,
            "https://example.com",
            "Test File",
            "mp3",
            Some("sent_file_id"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let files = get_sent_files(&conn, Some(10)).unwrap();
        assert!(!files.is_empty());
        assert_eq!(files[0].file_id, "sent_file_id");
    }

    // ==================== Expire Subscriptions Test ====================

    #[test]
    fn test_expire_old_subscriptions() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 12510, None).unwrap();

        // Set expired subscription
        conn.execute(
            "UPDATE subscriptions SET plan = 'premium', expires_at = datetime('now', '-1 day') WHERE user_id = 12510",
            [],
        )
        .unwrap();

        let count = expire_old_subscriptions(&conn).unwrap();
        assert_eq!(count, 1);

        let user = get_user(&conn, 12510).unwrap().unwrap();
        assert_eq!(user.plan, Plan::Free);
    }

    // ==================== Connection Pool Tests ====================

    #[test]
    fn test_create_pool_and_get_connection() {
        let temp_file = NamedTempFile::new().unwrap();
        let db_path = temp_file.path().to_str().unwrap();

        let pool = create_pool(db_path).unwrap();
        let conn = get_connection(&pool);
        assert!(conn.is_ok());
    }

    #[test]
    fn test_multiple_connections() {
        let pool = setup_test_db();

        let conn1 = get_connection(&pool);
        let conn2 = get_connection(&pool);

        assert!(conn1.is_ok());
        assert!(conn2.is_ok());
    }

    // ==================== Bot Assets Tests ====================

    #[test]
    fn test_get_bot_asset_nonexistent() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        let result = get_bot_asset(&conn, "nonexistent_key").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_set_and_get_bot_asset() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        set_bot_asset(&conn, "ringtone_instruction_iphone_1", "file_id_abc123").unwrap();

        let result = get_bot_asset(&conn, "ringtone_instruction_iphone_1").unwrap();
        assert_eq!(result, Some("file_id_abc123".to_string()));
    }

    #[test]
    fn test_set_bot_asset_upserts_existing_key() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        set_bot_asset(&conn, "key1", "first_value").unwrap();
        set_bot_asset(&conn, "key1", "updated_value").unwrap();

        let result = get_bot_asset(&conn, "key1").unwrap();
        assert_eq!(result, Some("updated_value".to_string()));
    }

    #[test]
    fn test_multiple_bot_assets_are_independent() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        for i in 1..=6 {
            let key = format!("ringtone_instruction_iphone_{}", i);
            let fid = format!("file_id_iphone_{}", i);
            set_bot_asset(&conn, &key, &fid).unwrap();
        }

        for i in 1..=6 {
            let key = format!("ringtone_instruction_iphone_{}", i);
            let expected = format!("file_id_iphone_{}", i);
            let result = get_bot_asset(&conn, &key).unwrap();
            assert_eq!(result, Some(expected), "Mismatch at step {}", i);
        }
    }

    #[test]
    fn test_iphone_and_android_assets_are_independent() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        set_bot_asset(&conn, "ringtone_instruction_iphone_1", "iphone_fid").unwrap();
        set_bot_asset(&conn, "ringtone_instruction_android_1", "android_fid").unwrap();

        assert_eq!(
            get_bot_asset(&conn, "ringtone_instruction_iphone_1").unwrap(),
            Some("iphone_fid".to_string())
        );
        assert_eq!(
            get_bot_asset(&conn, "ringtone_instruction_android_1").unwrap(),
            Some("android_fid".to_string())
        );
    }

    #[test]
    fn test_all_cached_detection_logic() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();
        let prefix = "ringtone_instruction_iphone_";
        let total = 3;

        // Before setting: none cached → not all_cached
        let cached: Vec<Option<String>> = (1..=total)
            .map(|i| get_bot_asset(&conn, &format!("{}{}", prefix, i)).ok().flatten())
            .collect();
        let all_cached = total > 0 && cached.iter().all(|id| id.is_some());
        assert!(!all_cached, "Should not be all_cached when nothing stored");

        // Set only 2 out of 3 — still not all_cached
        set_bot_asset(&conn, &format!("{}1", prefix), "fid_1").unwrap();
        set_bot_asset(&conn, &format!("{}2", prefix), "fid_2").unwrap();
        let cached: Vec<Option<String>> = (1..=total)
            .map(|i| get_bot_asset(&conn, &format!("{}{}", prefix, i)).ok().flatten())
            .collect();
        let all_cached = total > 0 && cached.iter().all(|id| id.is_some());
        assert!(!all_cached, "Should not be all_cached when only 2/3 set");

        // Set the last one — now all_cached
        set_bot_asset(&conn, &format!("{}3", prefix), "fid_3").unwrap();
        let cached: Vec<Option<String>> = (1..=total)
            .map(|i| get_bot_asset(&conn, &format!("{}{}", prefix, i)).ok().flatten())
            .collect();
        let all_cached = total > 0 && cached.iter().all(|id| id.is_some());
        assert!(all_cached, "Should be all_cached when all 3 set");
    }

    #[test]
    fn test_all_cached_zero_total_is_false() {
        // Edge case: if there are no images (total=0), all_cached must be false
        let total = 0usize;
        let cached: Vec<Option<String>> = vec![];
        let all_cached = total > 0 && cached.iter().all(|id| id.is_some());
        assert!(!all_cached, "all_cached with total=0 must be false");
    }

    #[test]
    fn test_get_user_counts_no_ambiguous_column() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        // Create a few users with different plans
        create_user(&conn, 1001, Some("user_free".to_string())).unwrap();
        create_user(&conn, 1002, Some("user_premium".to_string())).unwrap();
        // Upgrade one user to premium via subscription
        conn.execute(
            "INSERT OR REPLACE INTO subscriptions (user_id, plan, expires_at)
             VALUES (1002, 'premium', datetime('now', '+30 days'))",
            [],
        )
        .unwrap();

        // This used to crash with "ambiguous column name: plan"
        let counts = get_user_counts(&conn).unwrap();
        assert_eq!(counts.total, 2);
        assert!(counts.free >= 1);
        assert!(counts.premium >= 1);
    }

    #[test]
    fn test_get_users_paginated_no_ambiguous_column() {
        let pool = setup_test_db();
        let conn = get_connection(&pool).unwrap();

        create_user(&conn, 2001, Some("pag_user1".to_string())).unwrap();
        create_user(&conn, 2002, Some("pag_user2".to_string())).unwrap();

        // Should not crash with ambiguous column errors
        let (users, total) = get_users_paginated(&conn, None, 0, 10).unwrap();
        assert_eq!(total, 2);
        assert_eq!(users.len(), 2);

        let (filtered, count) = get_users_paginated(&conn, Some("free"), 0, 10).unwrap();
        assert_eq!(count, 2);
        assert_eq!(filtered.len(), 2);
    }
}
