use serde_json::Value as JsonValue;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SharePageRecord {
    pub id: String,
    pub youtube_url: String,
    pub title: String,
    pub artist: Option<String>,
    pub thumbnail_url: Option<String>,
    pub duration_secs: Option<i64>,
    /// Maps to the Postgres column `streaming_links` (sqlx column name differs from field).
    #[sqlx(rename = "streaming_links")]
    pub streaming_links_json: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct QueueTaskInput<'a> {
    pub task_id: &'a str,
    pub user_id: i64,
    pub url: &'a str,
    pub message_id: Option<i32>,
    pub format: &'a str,
    pub is_video: bool,
    pub video_quality: Option<&'a str>,
    pub audio_bitrate: Option<&'a str>,
    pub time_range_start: Option<&'a str>,
    pub time_range_end: Option<&'a str>,
    pub carousel_mask: Option<u32>,
    /// Whether the user toggled "📝 Lyrics ON" before queuing. Persisted via
    /// the `task_queue.with_lyrics` column (V47); plumbed back to
    /// `download_and_send_audio` so lyrics fetch fires even when the
    /// download lands on a cache-hit path that doesn't go through the
    /// in-memory `dl:mp3+lyr:` callback.
    pub with_lyrics: bool,
    pub priority: i32,
    pub idempotency_key: &'a str,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PreviewContext {
    pub original_message_id: Option<i32>,
    pub time_range: Option<(String, String)>,
    pub burn_sub_lang: Option<String>,
    pub audio_lang: Option<String>,
    pub speed: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct ContentSubscriptionRecord {
    pub id: i64,
    pub user_id: i64,
    pub source_type: String,
    pub source_id: String,
    pub display_name: String,
    pub watch_mask: u32,
    pub last_seen_state: Option<JsonValue>,
    pub source_meta: Option<JsonValue>,
    pub is_active: bool,
    pub last_checked_at: Option<String>,
    pub last_error: Option<String>,
    pub consecutive_errors: u32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct ContentSourceGroup {
    pub source_type: String,
    pub source_id: String,
    pub combined_mask: u32,
    pub subscriptions: Vec<ContentSubscriptionRecord>,
}
