use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use rusqlite::OptionalExtension;
use serde_json::Value as JsonValue;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};

use crate::core::config::{self, DatabaseDriver};
use crate::core::types::Plan;
use crate::download::audio_effects::{AudioEffectSession, MorphProfile};
use crate::storage::db;
use crate::storage::db::{
    AudioCutSession, Charge, CookiesUploadSession, CutEntry, DbConnection, DbPool, DownloadHistoryEntry, EnqueueResult,
    ErrorLogEntry, GlobalStats, PlayerSession, Playlist, PlaylistItem, SentFile, SubtitleStyle, SyncedPlaylist,
    SyncedTrack, TaskQueueEntry, User, UserCounts, UserStats, UserVault, VideoClipSession,
};
use crate::storage::uploads::{self, NewUpload, UploadEntry};
use crate::timestamps::{TimestampSource, VideoTimestamp};

#[derive(Debug, Clone)]
pub struct SharePageRecord {
    pub id: String,
    pub youtube_url: String,
    pub title: String,
    pub artist: Option<String>,
    pub thumbnail_url: Option<String>,
    pub duration_secs: Option<i64>,
    pub streaming_links_json: Option<String>,
    pub created_at: String,
}

const POSTGRES_BOOTSTRAP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS users (
    telegram_id BIGINT PRIMARY KEY,
    username TEXT,
    plan TEXT NOT NULL DEFAULT 'free',
    download_format TEXT NOT NULL DEFAULT 'mp3',
    download_subtitles INTEGER NOT NULL DEFAULT 0,
    video_quality TEXT NOT NULL DEFAULT 'best',
    audio_bitrate TEXT NOT NULL DEFAULT '320k',
    language TEXT NOT NULL DEFAULT 'en',
    send_as_document INTEGER NOT NULL DEFAULT 0,
    send_audio_as_document INTEGER NOT NULL DEFAULT 0,
    burn_subtitles INTEGER NOT NULL DEFAULT 0,
    progress_bar_style TEXT NOT NULL DEFAULT 'classic',
    subtitle_font_size TEXT NOT NULL DEFAULT 'medium',
    subtitle_text_color TEXT NOT NULL DEFAULT 'white',
    subtitle_outline_color TEXT NOT NULL DEFAULT 'black',
    subtitle_outline_width INTEGER NOT NULL DEFAULT 2,
    subtitle_shadow INTEGER NOT NULL DEFAULT 1,
    subtitle_position TEXT NOT NULL DEFAULT 'bottom',
    is_blocked INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS subscriptions (
    user_id BIGINT PRIMARY KEY REFERENCES users(telegram_id) ON DELETE CASCADE,
    plan TEXT NOT NULL DEFAULT 'free',
    expires_at TIMESTAMPTZ,
    telegram_charge_id TEXT,
    is_recurring INTEGER NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS charges (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(telegram_id) ON DELETE CASCADE,
    plan TEXT NOT NULL,
    telegram_charge_id TEXT NOT NULL,
    provider_charge_id TEXT,
    currency TEXT NOT NULL,
    total_amount BIGINT NOT NULL,
    invoice_payload TEXT NOT NULL,
    is_recurring INTEGER NOT NULL DEFAULT 0,
    is_first_recurring INTEGER NOT NULL DEFAULT 0,
    subscription_expiration_date TIMESTAMPTZ,
    payment_date TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_charges_user_id
    ON charges(user_id, payment_date DESC);

CREATE TABLE IF NOT EXISTS feedback_messages (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(telegram_id) ON DELETE CASCADE,
    username TEXT,
    first_name TEXT NOT NULL,
    message TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'new',
    admin_reply TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    replied_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_feedback_messages_status
    ON feedback_messages(status, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_feedback_messages_user_id
    ON feedback_messages(user_id, created_at DESC);

CREATE TABLE IF NOT EXISTS request_history (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(telegram_id) ON DELETE CASCADE,
    request_text TEXT NOT NULL,
    timestamp TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_request_history_user_timestamp
    ON request_history(user_id, timestamp DESC);

CREATE TABLE IF NOT EXISTS alert_history (
    id BIGSERIAL PRIMARY KEY,
    alert_type TEXT NOT NULL,
    severity TEXT NOT NULL,
    message TEXT NOT NULL,
    metadata TEXT,
    triggered_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    resolved_at TIMESTAMPTZ,
    acknowledged INTEGER NOT NULL DEFAULT 0,
    acknowledged_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_alert_history_type
    ON alert_history(alert_type);
CREATE INDEX IF NOT EXISTS idx_alert_history_triggered
    ON alert_history(triggered_at DESC);
CREATE INDEX IF NOT EXISTS idx_alert_history_unresolved
    ON alert_history(alert_type, resolved_at);

CREATE TABLE IF NOT EXISTS error_log (
    id BIGSERIAL PRIMARY KEY,
    timestamp TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    user_id BIGINT,
    username TEXT,
    error_type TEXT NOT NULL,
    error_message TEXT NOT NULL,
    url TEXT,
    context TEXT,
    resolved INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_error_log_timestamp
    ON error_log(timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_error_log_user_id
    ON error_log(user_id);
CREATE INDEX IF NOT EXISTS idx_error_log_type
    ON error_log(error_type);
CREATE INDEX IF NOT EXISTS idx_error_log_period
    ON error_log(timestamp, error_type);

CREATE TABLE IF NOT EXISTS share_pages (
    id TEXT PRIMARY KEY,
    youtube_url TEXT NOT NULL,
    title TEXT NOT NULL,
    artist TEXT,
    thumbnail_url TEXT,
    duration_secs BIGINT,
    streaming_links TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS url_cache (
    id TEXT PRIMARY KEY,
    url TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_url_cache_expires_at
    ON url_cache(expires_at);

CREATE TABLE IF NOT EXISTS bot_assets (
    key TEXT PRIMARY KEY,
    file_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS content_subscriptions (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(telegram_id) ON DELETE CASCADE,
    source_type TEXT NOT NULL,
    source_id TEXT NOT NULL,
    display_name TEXT NOT NULL DEFAULT '',
    watch_mask INTEGER NOT NULL DEFAULT 3,
    last_seen_state TEXT,
    source_meta TEXT,
    is_active INTEGER NOT NULL DEFAULT 1,
    last_checked_at TIMESTAMPTZ,
    last_error TEXT,
    consecutive_errors INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(user_id, source_type, source_id)
);

CREATE INDEX IF NOT EXISTS idx_content_subs_active
    ON content_subscriptions(is_active, last_checked_at);
CREATE INDEX IF NOT EXISTS idx_content_subs_user
    ON content_subscriptions(user_id, is_active);
CREATE INDEX IF NOT EXISTS idx_content_subs_source
    ON content_subscriptions(source_type, source_id, is_active);

CREATE TABLE IF NOT EXISTS uploads (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(telegram_id) ON DELETE CASCADE,
    original_filename TEXT,
    title TEXT NOT NULL,
    media_type TEXT NOT NULL,
    file_format TEXT,
    file_id TEXT NOT NULL,
    file_unique_id TEXT,
    file_size BIGINT,
    duration BIGINT,
    width INTEGER,
    height INTEGER,
    mime_type TEXT,
    message_id INTEGER,
    chat_id BIGINT,
    uploaded_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    thumbnail_file_id TEXT
);

CREATE INDEX IF NOT EXISTS idx_uploads_user_id
    ON uploads(user_id);
CREATE INDEX IF NOT EXISTS idx_uploads_uploaded_at
    ON uploads(uploaded_at DESC);
CREATE INDEX IF NOT EXISTS idx_uploads_media_type
    ON uploads(media_type);
CREATE INDEX IF NOT EXISTS idx_uploads_file_unique_id
    ON uploads(file_unique_id);

CREATE TABLE IF NOT EXISTS download_history (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(telegram_id) ON DELETE CASCADE,
    url TEXT NOT NULL,
    title TEXT NOT NULL,
    format TEXT NOT NULL,
    downloaded_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    file_id TEXT,
    author TEXT,
    file_size BIGINT,
    duration BIGINT,
    video_quality TEXT,
    audio_bitrate TEXT,
    bot_api_url TEXT,
    bot_api_is_local INTEGER NOT NULL DEFAULT 0,
    source_id BIGINT,
    part_index INTEGER,
    category TEXT,
    message_id INTEGER,
    chat_id BIGINT
);

CREATE INDEX IF NOT EXISTS idx_download_history_user_id
    ON download_history(user_id);
CREATE INDEX IF NOT EXISTS idx_download_history_downloaded_at
    ON download_history(downloaded_at DESC);
CREATE INDEX IF NOT EXISTS idx_download_history_url_format_api
    ON download_history(url, format, bot_api_is_local, downloaded_at DESC);

CREATE TABLE IF NOT EXISTS user_categories (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(telegram_id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(user_id, name)
);

CREATE INDEX IF NOT EXISTS idx_user_categories_user_id
    ON user_categories(user_id, name);

CREATE TABLE IF NOT EXISTS video_timestamps (
    id BIGSERIAL PRIMARY KEY,
    download_id BIGINT NOT NULL REFERENCES download_history(id) ON DELETE CASCADE,
    source TEXT NOT NULL,
    time_seconds BIGINT NOT NULL,
    end_seconds BIGINT,
    label TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_video_timestamps_download_id
    ON video_timestamps(download_id, time_seconds);

CREATE TABLE IF NOT EXISTS cuts (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(telegram_id) ON DELETE CASCADE,
    original_url TEXT NOT NULL,
    source_kind TEXT NOT NULL,
    source_id BIGINT NOT NULL,
    output_kind TEXT NOT NULL DEFAULT 'clip',
    segments_json TEXT NOT NULL,
    segments_text TEXT NOT NULL,
    title TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    file_id TEXT,
    file_size BIGINT,
    duration BIGINT,
    video_quality TEXT,
    message_id INTEGER,
    chat_id BIGINT
);

CREATE INDEX IF NOT EXISTS idx_cuts_user_id
    ON cuts(user_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_cuts_source
    ON cuts(user_id, source_kind, source_id);

CREATE TABLE IF NOT EXISTS task_queue (
    id TEXT PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(telegram_id) ON DELETE CASCADE,
    url TEXT NOT NULL,
    message_id INTEGER,
    format TEXT NOT NULL,
    is_video INTEGER NOT NULL DEFAULT 0,
    video_quality TEXT,
    audio_bitrate TEXT,
    time_range_start TEXT,
    time_range_end TEXT,
    carousel_mask INTEGER,
    priority INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'pending',
    error_message TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0,
    idempotency_key TEXT,
    worker_id TEXT,
    leased_at TIMESTAMPTZ,
    lease_expires_at TIMESTAMPTZ,
    last_heartbeat_at TIMESTAMPTZ,
    execute_at TIMESTAMPTZ,
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_task_queue_runnable
    ON task_queue(status, priority DESC, created_at ASC);
CREATE INDEX IF NOT EXISTS idx_task_queue_lease_expiry
    ON task_queue(status, lease_expires_at);
CREATE INDEX IF NOT EXISTS idx_task_queue_user_pending
    ON task_queue(user_id, status, created_at ASC);
CREATE UNIQUE INDEX IF NOT EXISTS idx_task_queue_active_idempotency
    ON task_queue(idempotency_key)
    WHERE idempotency_key IS NOT NULL
      AND status IN ('pending', 'leased', 'processing', 'uploading');

CREATE TABLE IF NOT EXISTS processed_updates (
    bot_id BIGINT NOT NULL,
    update_id BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (bot_id, update_id)
);

CREATE INDEX IF NOT EXISTS idx_processed_updates_created_at
    ON processed_updates(created_at);

CREATE TABLE IF NOT EXISTS audio_effect_sessions (
    id TEXT PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(telegram_id) ON DELETE CASCADE,
    original_file_path TEXT NOT NULL,
    current_file_path TEXT NOT NULL,
    telegram_file_id TEXT,
    original_message_id INTEGER NOT NULL,
    title TEXT NOT NULL,
    duration INTEGER NOT NULL,
    pitch_semitones SMALLINT NOT NULL DEFAULT 0,
    tempo_factor DOUBLE PRECISION NOT NULL DEFAULT 1.0,
    bass_gain_db SMALLINT NOT NULL DEFAULT 0,
    morph_profile TEXT NOT NULL DEFAULT 'none',
    version INTEGER NOT NULL DEFAULT 0,
    processing INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_audio_effect_sessions_user_message
    ON audio_effect_sessions(user_id, original_message_id);
CREATE INDEX IF NOT EXISTS idx_audio_effect_sessions_expires_at
    ON audio_effect_sessions(expires_at);

CREATE TABLE IF NOT EXISTS audio_cut_sessions (
    id TEXT PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(telegram_id) ON DELETE CASCADE,
    audio_session_id TEXT NOT NULL REFERENCES audio_effect_sessions(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_audio_cut_sessions_user_expires
    ON audio_cut_sessions(user_id, expires_at DESC);

CREATE TABLE IF NOT EXISTS video_clip_sessions (
    id TEXT PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(telegram_id) ON DELETE CASCADE,
    source_download_id BIGINT NOT NULL,
    source_kind TEXT NOT NULL DEFAULT 'download',
    source_id BIGINT NOT NULL,
    original_url TEXT NOT NULL DEFAULT '',
    output_kind TEXT NOT NULL DEFAULT 'cut',
    created_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    subtitle_lang TEXT
);

CREATE INDEX IF NOT EXISTS idx_video_clip_sessions_user_expires
    ON video_clip_sessions(user_id, expires_at DESC);

CREATE TABLE IF NOT EXISTS cookies_upload_sessions (
    id TEXT PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(telegram_id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_cookies_upload_sessions_user_expires
    ON cookies_upload_sessions(user_id, expires_at DESC);

CREATE TABLE IF NOT EXISTS ig_cookies_upload_sessions (
    id TEXT PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(telegram_id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_ig_cookies_upload_sessions_user_expires
    ON ig_cookies_upload_sessions(user_id, expires_at DESC);

CREATE TABLE IF NOT EXISTS lyrics_sessions (
    id TEXT PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(telegram_id) ON DELETE CASCADE,
    artist TEXT NOT NULL,
    title TEXT NOT NULL,
    sections_json TEXT NOT NULL,
    has_structure INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_lyrics_sessions_expires_at
    ON lyrics_sessions(expires_at);

CREATE TABLE IF NOT EXISTS player_sessions (
    user_id BIGINT PRIMARY KEY REFERENCES users(telegram_id) ON DELETE CASCADE,
    playlist_id BIGINT NOT NULL,
    current_position INTEGER NOT NULL DEFAULT 0,
    is_shuffle INTEGER NOT NULL DEFAULT 0,
    player_message_id INTEGER,
    sticker_message_id INTEGER,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS player_messages (
    user_id BIGINT NOT NULL REFERENCES users(telegram_id) ON DELETE CASCADE,
    message_id INTEGER NOT NULL,
    PRIMARY KEY (user_id, message_id)
);

CREATE INDEX IF NOT EXISTS idx_player_messages_user
    ON player_messages(user_id);

CREATE TABLE IF NOT EXISTS playlists (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(telegram_id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT,
    is_public INTEGER NOT NULL DEFAULT 0,
    share_token TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_playlists_user_updated
    ON playlists(user_id, updated_at DESC);
CREATE UNIQUE INDEX IF NOT EXISTS idx_playlists_share_token
    ON playlists(share_token)
    WHERE share_token IS NOT NULL;

CREATE TABLE IF NOT EXISTS playlist_items (
    id BIGSERIAL PRIMARY KEY,
    playlist_id BIGINT NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    download_history_id BIGINT,
    title TEXT NOT NULL,
    artist TEXT,
    url TEXT NOT NULL,
    duration_secs INTEGER,
    file_id TEXT,
    source TEXT NOT NULL,
    added_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_playlist_items_playlist_position
    ON playlist_items(playlist_id, position);

CREATE TABLE IF NOT EXISTS synced_playlists (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(telegram_id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT,
    source_url TEXT NOT NULL,
    source_platform TEXT NOT NULL,
    track_count INTEGER NOT NULL DEFAULT 0,
    matched_count INTEGER NOT NULL DEFAULT 0,
    not_found_count INTEGER NOT NULL DEFAULT 0,
    sync_enabled INTEGER NOT NULL DEFAULT 0,
    last_synced_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_synced_playlists_user_created
    ON synced_playlists(user_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_synced_playlists_user_url
    ON synced_playlists(user_id, source_url);

CREATE TABLE IF NOT EXISTS synced_tracks (
    id BIGSERIAL PRIMARY KEY,
    playlist_id BIGINT NOT NULL REFERENCES synced_playlists(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    title TEXT NOT NULL,
    artist TEXT,
    duration_secs INTEGER,
    external_id TEXT,
    source_url TEXT,
    resolved_url TEXT,
    import_status TEXT NOT NULL DEFAULT 'pending',
    file_id TEXT,
    added_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_synced_tracks_playlist
    ON synced_tracks(playlist_id, position);
CREATE INDEX IF NOT EXISTS idx_synced_tracks_external
    ON synced_tracks(playlist_id, external_id);

CREATE TABLE IF NOT EXISTS new_category_sessions (
    user_id BIGINT PRIMARY KEY REFERENCES users(telegram_id) ON DELETE CASCADE,
    download_id BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS user_vaults (
    user_id BIGINT PRIMARY KEY REFERENCES users(telegram_id) ON DELETE CASCADE,
    channel_id BIGINT NOT NULL,
    channel_title TEXT,
    is_active INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS vault_cache (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(telegram_id) ON DELETE CASCADE,
    url TEXT NOT NULL,
    title TEXT,
    artist TEXT,
    duration_secs INTEGER,
    file_id TEXT NOT NULL,
    message_id BIGINT,
    file_size BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(user_id, url)
);

CREATE INDEX IF NOT EXISTS idx_vault_cache_lookup
    ON vault_cache(user_id, url);

CREATE TABLE IF NOT EXISTS search_sessions (
    user_id BIGINT PRIMARY KEY REFERENCES users(telegram_id) ON DELETE CASCADE,
    query TEXT NOT NULL,
    results_json TEXT NOT NULL,
    source TEXT NOT NULL,
    context_kind TEXT NOT NULL,
    playlist_id BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_search_sessions_created_at
    ON search_sessions(created_at);

CREATE TABLE IF NOT EXISTS prompt_sessions (
    user_id BIGINT NOT NULL REFERENCES users(telegram_id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    payload_json TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (user_id, kind)
);

CREATE INDEX IF NOT EXISTS idx_prompt_sessions_expires_at
    ON prompt_sessions(expires_at);

CREATE TABLE IF NOT EXISTS preview_contexts (
    user_id BIGINT NOT NULL,
    url TEXT NOT NULL,
    original_message_id INTEGER,
    time_range_start TEXT,
    time_range_end TEXT,
    burn_sub_lang TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (user_id, url)
);

CREATE INDEX IF NOT EXISTS idx_preview_contexts_expires_at
    ON preview_contexts(expires_at);
"#;

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
    pub priority: i32,
    pub idempotency_key: &'a str,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreviewContext {
    pub original_message_id: Option<i32>,
    pub time_range: Option<(String, String)>,
    pub burn_sub_lang: Option<String>,
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

fn upload_entry_from_pg_row(row: &sqlx::postgres::PgRow) -> UploadEntry {
    UploadEntry {
        id: row.get("id"),
        user_id: row.get("user_id"),
        original_filename: row.get("original_filename"),
        title: row.get("title"),
        media_type: row.get("media_type"),
        file_format: row.get("file_format"),
        file_id: row.get("file_id"),
        file_unique_id: row.get("file_unique_id"),
        file_size: row.get("file_size"),
        duration: row.get("duration"),
        width: row.get("width"),
        height: row.get("height"),
        mime_type: row.get("mime_type"),
        message_id: row.get("message_id"),
        chat_id: row.get("chat_id"),
        uploaded_at: row.get("uploaded_at"),
        thumbnail_file_id: row.get("thumbnail_file_id"),
    }
}

#[derive(Clone)]
pub enum SharedStorage {
    Sqlite { db_pool: Arc<DbPool> },
    Postgres { sqlite_pool: Arc<DbPool>, pg_pool: PgPool },
}

impl SharedStorage {
    pub async fn from_sqlite_pool(db_pool: Arc<DbPool>) -> Result<Arc<Self>> {
        match *config::DATABASE_DRIVER {
            DatabaseDriver::Sqlite => Ok(Arc::new(Self::Sqlite { db_pool })),
            DatabaseDriver::Postgres => {
                let database_url = config::DATABASE_URL
                    .clone()
                    .ok_or_else(|| anyhow!("DATABASE_URL must be set when DATABASE_DRIVER=postgres"))?;
                let pg_pool = PgPoolOptions::new()
                    .max_connections(20)
                    .acquire_timeout(Duration::from_secs(3))
                    .connect(&database_url)
                    .await
                    .context("connect postgres shared storage")?;
                sqlx::query(POSTGRES_BOOTSTRAP_SQL)
                    .execute(&pg_pool)
                    .await
                    .context("bootstrap postgres shared storage schema")?;
                Ok(Arc::new(Self::Postgres {
                    sqlite_pool: db_pool,
                    pg_pool,
                }))
            }
        }
    }

    pub fn sqlite_pool(&self) -> Arc<DbPool> {
        match self {
            Self::Sqlite { db_pool } => Arc::clone(db_pool),
            Self::Postgres { sqlite_pool, .. } => Arc::clone(sqlite_pool),
        }
    }

    pub fn is_postgres(&self) -> bool {
        matches!(self, Self::Postgres { .. })
    }

    pub async fn register_processed_update(&self, bot_id: i64, update_id: i64) -> Result<bool> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite register_processed_update connection")?;
                db::register_processed_update(&conn, bot_id, update_id).context("sqlite register_processed_update")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "INSERT INTO processed_updates (bot_id, update_id) VALUES ($1, $2)
                     ON CONFLICT DO NOTHING",
                )
                .bind(bot_id)
                .bind(update_id)
                .execute(pg_pool)
                .await
                .context("postgres register_processed_update")?
                .rows_affected();
                Ok(rows > 0)
            }
        }
    }

    pub async fn cleanup_old_processed_updates(&self, hours: i64) -> Result<u64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite cleanup_old_processed_updates connection")?;
                Ok(
                    db::cleanup_old_processed_updates(&conn, hours).context("sqlite cleanup_old_processed_updates")?
                        as u64,
                )
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "DELETE FROM processed_updates WHERE created_at < NOW() - ($1::bigint * INTERVAL '1 hour')",
                )
                .bind(hours)
                .execute(pg_pool)
                .await
                .context("postgres cleanup_old_processed_updates")?
                .rows_affected();
                Ok(rows)
            }
        }
    }

    pub async fn save_task_to_queue(&self, input: QueueTaskInput<'_>) -> Result<EnqueueResult> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite save_task_to_queue connection")?;
                db::save_task_to_queue(
                    &conn,
                    input.task_id,
                    input.user_id,
                    input.url,
                    input.message_id,
                    input.format,
                    input.is_video,
                    input.video_quality,
                    input.audio_bitrate,
                    input.time_range_start,
                    input.time_range_end,
                    input.carousel_mask,
                    input.priority,
                    input.idempotency_key,
                )
                .context("sqlite save_task_to_queue")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "INSERT INTO task_queue (
                        id, user_id, url, message_id, format, is_video, video_quality, audio_bitrate,
                        time_range_start, time_range_end, carousel_mask, priority, status, retry_count, idempotency_key
                     ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, 'pending', 0, $13)
                     ON CONFLICT DO NOTHING",
                )
                .bind(input.task_id)
                .bind(input.user_id)
                .bind(input.url)
                .bind(input.message_id)
                .bind(input.format)
                .bind(if input.is_video { 1_i32 } else { 0_i32 })
                .bind(input.video_quality)
                .bind(input.audio_bitrate)
                .bind(input.time_range_start)
                .bind(input.time_range_end)
                .bind(input.carousel_mask.map(|value| value as i32))
                .bind(input.priority)
                .bind(input.idempotency_key)
                .execute(pg_pool)
                .await
                .context("postgres save_task_to_queue")?
                .rows_affected();
                Ok(if rows == 0 {
                    EnqueueResult::Duplicate
                } else {
                    EnqueueResult::Enqueued
                })
            }
        }
    }

    pub async fn claim_next_task(&self, worker_id: &str, lease_seconds: i64) -> Result<Option<TaskQueueEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite claim_next_task connection")?;
                db::claim_next_task(&conn, worker_id, lease_seconds).context("sqlite claim_next_task")
            }
            Self::Postgres { pg_pool, .. } => {
                let mut tx = pg_pool.begin().await.context("postgres claim_next_task begin")?;
                let row = sqlx::query(
                    "WITH candidate AS (
                        SELECT id
                        FROM task_queue
                        WHERE status = 'pending'
                          AND (execute_at IS NULL OR execute_at <= NOW())
                        ORDER BY priority DESC, created_at ASC
                        FOR UPDATE SKIP LOCKED
                        LIMIT 1
                    )
                    UPDATE task_queue t
                    SET status = 'leased',
                        worker_id = $1,
                        leased_at = NOW(),
                        lease_expires_at = NOW() + ($2::bigint * INTERVAL '1 second'),
                        last_heartbeat_at = NOW(),
                        updated_at = NOW()
                    FROM candidate
                    WHERE t.id = candidate.id
                    RETURNING t.*",
                )
                .bind(worker_id)
                .bind(lease_seconds)
                .fetch_optional(&mut *tx)
                .await
                .context("postgres claim_next_task update")?;
                tx.commit().await.context("postgres claim_next_task commit")?;
                row.map(map_pg_task_queue_entry).transpose()
            }
        }
    }

    pub async fn heartbeat_worker_leases(&self, worker_id: &str, lease_seconds: i64) -> Result<u64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite heartbeat_worker_leases connection")?;
                Ok(db::heartbeat_worker_leases(&conn, worker_id, lease_seconds)
                    .context("sqlite heartbeat_worker_leases")? as u64)
            }
            Self::Postgres { pg_pool, .. } => Ok(sqlx::query(
                "UPDATE task_queue
                 SET lease_expires_at = NOW() + ($1::bigint * INTERVAL '1 second'),
                     last_heartbeat_at = NOW(),
                     updated_at = NOW()
                 WHERE worker_id = $2
                   AND status IN ('leased', 'processing', 'uploading')",
            )
            .bind(lease_seconds)
            .bind(worker_id)
            .execute(pg_pool)
            .await
            .context("postgres heartbeat_worker_leases")?
            .rows_affected()),
        }
    }

    pub async fn mark_task_processing(&self, task_id: &str, worker_id: &str) -> Result<()> {
        self.run_task_status_update(
            "UPDATE task_queue
             SET status = 'processing',
                 started_at = COALESCE(started_at, NOW()),
                 updated_at = NOW()
             WHERE id = $1
               AND worker_id = $2
               AND status IN ('leased', 'processing', 'uploading')",
            task_id,
            worker_id,
        )
        .await
    }

    pub async fn mark_task_uploading(&self, task_id: &str, worker_id: &str) -> Result<()> {
        self.run_task_status_update(
            "UPDATE task_queue
             SET status = 'uploading',
                 updated_at = NOW()
             WHERE id = $1
               AND worker_id = $2
               AND status IN ('leased', 'processing', 'uploading')",
            task_id,
            worker_id,
        )
        .await
    }

    pub async fn mark_task_completed(&self, task_id: &str, worker_id: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite mark_task_completed connection")?;
                db::mark_task_completed(&conn, task_id, worker_id).context("sqlite mark_task_completed")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE task_queue
                     SET status = 'completed',
                         worker_id = NULL,
                         leased_at = NULL,
                         lease_expires_at = NULL,
                         last_heartbeat_at = NULL,
                         finished_at = NOW(),
                         updated_at = NOW()
                     WHERE id = $1
                       AND worker_id = $2
                       AND status IN ('leased', 'processing', 'uploading')",
                )
                .bind(task_id)
                .bind(worker_id)
                .execute(pg_pool)
                .await
                .context("postgres mark_task_completed")?;
                Ok(())
            }
        }
    }

    pub async fn mark_task_failed(
        &self,
        task_id: &str,
        worker_id: &str,
        error_message: &str,
        retryable: bool,
        max_retries: i32,
    ) -> Result<bool> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite mark_task_failed connection")?;
                db::mark_task_failed(&conn, task_id, worker_id, error_message, retryable, max_retries)
                    .context("sqlite mark_task_failed")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query("SELECT retry_count + 1 AS next_retry_count FROM task_queue WHERE id = $1")
                    .bind(task_id)
                    .fetch_one(pg_pool)
                    .await
                    .context("postgres mark_task_failed select next_retry_count")?;
                let next_retry_count: i32 = row.get("next_retry_count");
                if !retryable || next_retry_count >= max_retries {
                    sqlx::query(
                        "UPDATE task_queue
                         SET status = 'dead_letter',
                             error_message = $1,
                             retry_count = retry_count + 1,
                             worker_id = NULL,
                             leased_at = NULL,
                             lease_expires_at = NULL,
                             last_heartbeat_at = NULL,
                             finished_at = NOW(),
                             updated_at = NOW()
                         WHERE id = $2
                           AND worker_id = $3",
                    )
                    .bind(error_message)
                    .bind(task_id)
                    .bind(worker_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres mark_task_failed dead_letter")?;
                    return Ok(false);
                }

                let delay_seconds = retry_delay_seconds(next_retry_count);
                sqlx::query(
                    "UPDATE task_queue
                     SET status = 'pending',
                         error_message = $1,
                         retry_count = retry_count + 1,
                         worker_id = NULL,
                         leased_at = NULL,
                         lease_expires_at = NULL,
                         last_heartbeat_at = NULL,
                         execute_at = NOW() + ($2::bigint * INTERVAL '1 second'),
                         updated_at = NOW()
                     WHERE id = $3
                       AND worker_id = $4",
                )
                .bind(error_message)
                .bind(delay_seconds)
                .bind(task_id)
                .bind(worker_id)
                .execute(pg_pool)
                .await
                .context("postgres mark_task_failed retryable")?;
                Ok(true)
            }
        }
    }

    pub async fn recover_expired_leases(&self, max_retries: i32) -> Result<u64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite recover_expired_leases connection")?;
                Ok(db::recover_expired_leases(&conn, max_retries).context("sqlite recover_expired_leases")? as u64)
            }
            Self::Postgres { pg_pool, .. } => Ok(sqlx::query(
                "UPDATE task_queue
                 SET status = CASE
                        WHEN retry_count + 1 >= $1 THEN 'dead_letter'
                        ELSE 'pending'
                     END,
                     retry_count = retry_count + 1,
                     error_message = COALESCE(error_message, 'Lease expired'),
                     worker_id = NULL,
                     leased_at = NULL,
                     lease_expires_at = NULL,
                     last_heartbeat_at = NULL,
                     execute_at = CASE
                        WHEN retry_count + 1 >= $1 THEN execute_at
                        ELSE NOW() + INTERVAL '30 seconds'
                     END,
                     finished_at = CASE
                        WHEN retry_count + 1 >= $1 THEN NOW()
                        ELSE finished_at
                     END,
                     updated_at = NOW()
                 WHERE status IN ('leased', 'processing', 'uploading')
                   AND lease_expires_at IS NOT NULL
                   AND lease_expires_at <= NOW()",
            )
            .bind(max_retries)
            .execute(pg_pool)
            .await
            .context("postgres recover_expired_leases")?
            .rows_affected()),
        }
    }

    pub async fn count_active_tasks(&self) -> Result<usize> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite count_active_tasks connection")?;
                db::count_active_tasks(&conn).context("sqlite count_active_tasks")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT COUNT(*)::bigint AS count
                     FROM task_queue
                     WHERE status IN ('pending', 'leased', 'processing', 'uploading')",
                )
                .fetch_one(pg_pool)
                .await
                .context("postgres count_active_tasks")?;
                Ok(row.get::<i64, _>("count") as usize)
            }
        }
    }

    pub async fn get_queue_position(&self, user_id: i64) -> Result<Option<usize>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_queue_position connection")?;
                db::get_queue_position(&conn, user_id).context("sqlite get_queue_position")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "WITH target AS (
                        SELECT priority, created_at
                        FROM task_queue
                        WHERE user_id = $1 AND status = 'pending'
                        ORDER BY priority DESC, created_at ASC
                        LIMIT 1
                    )
                    SELECT COUNT(*)::bigint + 1 AS position
                    FROM task_queue, target
                    WHERE status = 'pending'
                      AND (
                        task_queue.priority > target.priority OR
                        (task_queue.priority = target.priority AND task_queue.created_at < target.created_at)
                      )",
                )
                .bind(user_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_queue_position")?;
                Ok(row.map(|row| row.get::<i64, _>("position") as usize))
            }
        }
    }

    pub async fn get_pending_tasks_for_user(&self, user_id: i64) -> Result<Vec<TaskQueueEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_pending_tasks_for_user connection")?;
                db::get_pending_tasks_for_user(&conn, user_id).context("sqlite get_pending_tasks_for_user")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT *
                     FROM task_queue
                     WHERE user_id = $1
                       AND status = 'pending'
                     ORDER BY priority DESC, created_at ASC",
                )
                .bind(user_id)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_pending_tasks_for_user")?;
                rows.into_iter().map(map_pg_task_queue_entry).collect()
            }
        }
    }

    pub async fn get_user(&self, telegram_id: i64) -> Result<Option<User>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user connection")?;
                db::get_user(&conn, telegram_id).context("sqlite get_user")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT
                        u.telegram_id,
                        u.username,
                        COALESCE(s.plan, u.plan) AS plan,
                        u.download_format,
                        u.download_subtitles,
                        u.video_quality,
                        u.audio_bitrate,
                        u.language,
                        u.send_as_document,
                        u.send_audio_as_document,
                        CAST(s.expires_at AS TEXT) AS subscription_expires_at,
                        s.telegram_charge_id,
                        COALESCE(s.is_recurring, 0) AS is_recurring,
                        COALESCE(u.burn_subtitles, 0) AS burn_subtitles,
                        COALESCE(u.progress_bar_style, 'classic') AS progress_bar_style,
                        COALESCE(u.is_blocked, 0) AS is_blocked
                     FROM users u
                     LEFT JOIN subscriptions s ON s.user_id = u.telegram_id
                     WHERE u.telegram_id = $1",
                )
                .bind(telegram_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_user")?;
                row.map(map_pg_user).transpose()
            }
        }
    }

    pub async fn get_user_counts(&self) -> Result<UserCounts> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user_counts connection")?;
                db::get_user_counts(&conn).context("sqlite get_user_counts")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT
                        COALESCE(s.plan, u.plan) AS plan,
                        COALESCE(u.is_blocked, 0) AS is_blocked,
                        COUNT(*) AS count
                     FROM users u
                     LEFT JOIN subscriptions s ON s.user_id = u.telegram_id
                     GROUP BY COALESCE(s.plan, u.plan), COALESCE(u.is_blocked, 0)",
                )
                .fetch_all(pg_pool)
                .await
                .context("postgres get_user_counts")?;

                let mut counts = UserCounts::default();
                for row in rows {
                    let plan: String = row.get("plan");
                    let blocked = row.get::<i32, _>("is_blocked") != 0;
                    let count = row.get::<i64, _>("count") as usize;
                    counts.total += count;
                    if blocked {
                        counts.blocked += count;
                    }
                    match plan.as_str() {
                        "premium" => counts.premium += count,
                        "vip" => counts.vip += count,
                        _ => counts.free += count,
                    }
                }
                Ok(counts)
            }
        }
    }

    pub async fn get_users_paginated(
        &self,
        filter: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<User>, usize)> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_users_paginated connection")?;
                db::get_users_paginated(&conn, filter, offset, limit).context("sqlite get_users_paginated")
            }
            Self::Postgres { pg_pool, .. } => {
                let where_sql = match filter {
                    Some("free") => "WHERE COALESCE(s.plan, u.plan) = 'free'",
                    Some("premium") => "WHERE COALESCE(s.plan, u.plan) = 'premium'",
                    Some("vip") => "WHERE COALESCE(s.plan, u.plan) = 'vip'",
                    Some("blocked") => "WHERE COALESCE(u.is_blocked, 0) = 1",
                    _ => "",
                };

                let count_sql = format!(
                    "SELECT COUNT(*) AS count
                     FROM users u
                     LEFT JOIN subscriptions s ON s.user_id = u.telegram_id
                     {}",
                    where_sql
                );
                let total = sqlx::query(&count_sql)
                    .fetch_one(pg_pool)
                    .await
                    .context("postgres get_users_paginated count")?
                    .get::<i64, _>("count") as usize;

                let query_sql = format!(
                    "SELECT
                        u.telegram_id,
                        u.username,
                        COALESCE(s.plan, u.plan) AS plan,
                        u.download_format,
                        u.download_subtitles,
                        u.video_quality,
                        u.audio_bitrate,
                        u.language,
                        u.send_as_document,
                        u.send_audio_as_document,
                        CAST(s.expires_at AS TEXT) AS subscription_expires_at,
                        s.telegram_charge_id,
                        COALESCE(s.is_recurring, 0) AS is_recurring,
                        COALESCE(u.burn_subtitles, 0) AS burn_subtitles,
                        COALESCE(u.progress_bar_style, 'classic') AS progress_bar_style,
                        COALESCE(u.is_blocked, 0) AS is_blocked
                     FROM users u
                     LEFT JOIN subscriptions s ON s.user_id = u.telegram_id
                     {}
                     ORDER BY u.telegram_id
                     LIMIT $1 OFFSET $2",
                    where_sql
                );
                let rows = sqlx::query(&query_sql)
                    .bind(limit as i64)
                    .bind(offset as i64)
                    .fetch_all(pg_pool)
                    .await
                    .context("postgres get_users_paginated rows")?;
                let users = rows.into_iter().map(map_pg_user).collect::<Result<Vec<_>>>()?;
                Ok((users, total))
            }
        }
    }

    pub async fn search_users(&self, query: &str) -> Result<Vec<User>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite search_users connection")?;
                db::search_users(&conn, query).context("sqlite search_users")
            }
            Self::Postgres { pg_pool, .. } => {
                let pattern = format!("%{}%", query);
                let rows = sqlx::query(
                    "SELECT
                        u.telegram_id,
                        u.username,
                        COALESCE(s.plan, u.plan) AS plan,
                        u.download_format,
                        u.download_subtitles,
                        u.video_quality,
                        u.audio_bitrate,
                        u.language,
                        u.send_as_document,
                        u.send_audio_as_document,
                        CAST(s.expires_at AS TEXT) AS subscription_expires_at,
                        s.telegram_charge_id,
                        COALESCE(s.is_recurring, 0) AS is_recurring,
                        COALESCE(u.burn_subtitles, 0) AS burn_subtitles,
                        COALESCE(u.progress_bar_style, 'classic') AS progress_bar_style,
                        COALESCE(u.is_blocked, 0) AS is_blocked
                     FROM users u
                     LEFT JOIN subscriptions s ON s.user_id = u.telegram_id
                     WHERE CAST(u.telegram_id AS TEXT) LIKE $1
                        OR COALESCE(u.username, '') LIKE $1
                     ORDER BY u.telegram_id
                     LIMIT 20",
                )
                .bind(pattern)
                .fetch_all(pg_pool)
                .await
                .context("postgres search_users")?;
                rows.into_iter().map(map_pg_user).collect()
            }
        }
    }

    pub async fn get_all_users(&self) -> Result<Vec<User>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_all_users connection")?;
                db::get_all_users(&conn).context("sqlite get_all_users")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT
                        u.telegram_id,
                        u.username,
                        COALESCE(s.plan, u.plan) AS plan,
                        u.download_format,
                        u.download_subtitles,
                        u.video_quality,
                        u.audio_bitrate,
                        u.language,
                        u.send_as_document,
                        u.send_audio_as_document,
                        CAST(s.expires_at AS TEXT) AS subscription_expires_at,
                        s.telegram_charge_id,
                        COALESCE(s.is_recurring, 0) AS is_recurring,
                        COALESCE(u.burn_subtitles, 0) AS burn_subtitles,
                        COALESCE(u.progress_bar_style, 'classic') AS progress_bar_style,
                        COALESCE(u.is_blocked, 0) AS is_blocked
                     FROM users u
                     LEFT JOIN subscriptions s ON s.user_id = u.telegram_id
                     ORDER BY u.telegram_id",
                )
                .fetch_all(pg_pool)
                .await
                .context("postgres get_all_users")?;
                rows.into_iter().map(map_pg_user).collect()
            }
        }
    }

    pub async fn get_sent_files(&self, limit: Option<i32>) -> Result<Vec<SentFile>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_sent_files connection")?;
                db::get_sent_files(&conn, limit).context("sqlite get_sent_files")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT dh.id, dh.user_id, u.username, dh.url, dh.title, dh.format,
                            CAST(dh.downloaded_at AS TEXT) AS downloaded_at, dh.file_id,
                            dh.message_id, dh.chat_id
                     FROM download_history dh
                     LEFT JOIN users u ON u.telegram_id = dh.user_id
                     WHERE dh.file_id IS NOT NULL
                     ORDER BY dh.downloaded_at DESC
                     LIMIT $1",
                )
                .bind(i64::from(limit.unwrap_or(50)))
                .fetch_all(pg_pool)
                .await
                .context("postgres get_sent_files")?;
                rows.into_iter().map(map_pg_sent_file).collect()
            }
        }
    }

    pub async fn is_user_blocked(&self, telegram_id: i64) -> Result<bool> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite is_user_blocked connection")?;
                db::is_user_blocked(&conn, telegram_id).context("sqlite is_user_blocked")
            }
            Self::Postgres { pg_pool, .. } => {
                let blocked =
                    sqlx::query_scalar::<_, i32>("SELECT COALESCE(is_blocked, 0) FROM users WHERE telegram_id = $1")
                        .bind(telegram_id)
                        .fetch_optional(pg_pool)
                        .await
                        .context("postgres is_user_blocked")?
                        .unwrap_or(0);
                Ok(blocked != 0)
            }
        }
    }

    pub async fn set_user_blocked(&self, telegram_id: i64, blocked: bool) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite set_user_blocked connection")?;
                db::set_user_blocked(&conn, telegram_id, blocked).context("sqlite set_user_blocked")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("UPDATE users SET is_blocked = $2, updated_at = NOW() WHERE telegram_id = $1")
                    .bind(telegram_id)
                    .bind(if blocked { 1 } else { 0 })
                    .execute(pg_pool)
                    .await
                    .context("postgres set_user_blocked")?;
                Ok(())
            }
        }
    }

    pub async fn expire_old_subscriptions(&self) -> Result<usize> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite expire_old_subscriptions connection")?;
                db::expire_old_subscriptions(&conn).context("sqlite expire_old_subscriptions")
            }
            Self::Postgres { pg_pool, .. } => {
                let result = sqlx::query(
                    "UPDATE subscriptions
                     SET plan = 'free',
                         expires_at = NULL,
                         telegram_charge_id = NULL,
                         is_recurring = FALSE,
                         updated_at = NOW()
                     WHERE plan != 'free'
                       AND expires_at IS NOT NULL
                       AND expires_at <= NOW()",
                )
                .execute(pg_pool)
                .await
                .context("postgres expire_old_subscriptions")?;
                Ok(result.rows_affected() as usize)
            }
        }
    }

    pub async fn update_user_plan_with_expiry(&self, telegram_id: i64, plan: &str, days: Option<i32>) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite update_user_plan_with_expiry connection")?;
                db::update_user_plan_with_expiry(&conn, telegram_id, plan, days)
                    .context("sqlite update_user_plan_with_expiry")
            }
            Self::Postgres { pg_pool, .. } => {
                let expires_at = days.map(|days| chrono::Utc::now() + chrono::Duration::days(i64::from(days)));
                sqlx::query(
                    "INSERT INTO subscriptions (user_id, plan, expires_at, telegram_charge_id, is_recurring, updated_at)
                     VALUES ($1, $2, $3, NULL, 0, NOW())
                     ON CONFLICT (user_id) DO UPDATE SET
                        plan = EXCLUDED.plan,
                        expires_at = EXCLUDED.expires_at,
                        telegram_charge_id = NULL,
                        is_recurring = 0,
                        updated_at = NOW()",
                )
                .bind(telegram_id)
                .bind(plan)
                .bind(expires_at)
                .execute(pg_pool)
                .await
                .context("postgres update_user_plan_with_expiry subscriptions")?;
                sqlx::query("UPDATE users SET plan = $2, updated_at = NOW() WHERE telegram_id = $1")
                    .bind(telegram_id)
                    .bind(plan)
                    .execute(pg_pool)
                    .await
                    .context("postgres update_user_plan_with_expiry users")?;
                Ok(())
            }
        }
    }

    pub async fn save_charge(
        &self,
        user_id: i64,
        plan: &str,
        telegram_charge_id: &str,
        provider_charge_id: Option<&str>,
        currency: &str,
        total_amount: i64,
        invoice_payload: &str,
        is_recurring: bool,
        is_first_recurring: bool,
        subscription_expiration_date: Option<&str>,
    ) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite save_charge connection")?;
                db::save_charge(
                    &conn,
                    user_id,
                    plan,
                    telegram_charge_id,
                    provider_charge_id,
                    currency,
                    total_amount,
                    invoice_payload,
                    is_recurring,
                    is_first_recurring,
                    subscription_expiration_date,
                )
                .context("sqlite save_charge")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "INSERT INTO charges (
                        user_id, plan, telegram_charge_id, provider_charge_id, currency,
                        total_amount, invoice_payload, is_recurring, is_first_recurring,
                        subscription_expiration_date
                     ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                     RETURNING id",
                )
                .bind(user_id)
                .bind(plan)
                .bind(telegram_charge_id)
                .bind(provider_charge_id)
                .bind(currency)
                .bind(total_amount)
                .bind(invoice_payload)
                .bind(i32::from(is_recurring))
                .bind(i32::from(is_first_recurring))
                .bind(subscription_expiration_date)
                .fetch_one(pg_pool)
                .await
                .context("postgres save_charge")?;
                Ok(row.get::<i64, _>("id"))
            }
        }
    }

    pub async fn save_feedback(
        &self,
        user_id: i64,
        username: Option<&str>,
        first_name: &str,
        message: &str,
    ) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite save_feedback connection")?;
                db::save_feedback(&conn, user_id, username, first_name, message).context("sqlite save_feedback")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "INSERT INTO feedback_messages (user_id, username, first_name, message, status)
                     VALUES ($1, $2, $3, $4, 'new')
                     RETURNING id",
                )
                .bind(user_id)
                .bind(username)
                .bind(first_name)
                .bind(message)
                .fetch_one(pg_pool)
                .await
                .context("postgres save_feedback")?;
                Ok(row.get::<i64, _>("id"))
            }
        }
    }

    pub async fn get_user_charges(&self, user_id: i64) -> Result<Vec<Charge>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user_charges connection")?;
                db::get_user_charges(&conn, user_id).context("sqlite get_user_charges")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, user_id, plan, telegram_charge_id, provider_charge_id, currency,
                            total_amount, invoice_payload, is_recurring, is_first_recurring,
                            CAST(subscription_expiration_date AS TEXT) AS subscription_expiration_date,
                            CAST(payment_date AS TEXT) AS payment_date,
                            CAST(created_at AS TEXT) AS created_at
                     FROM charges
                     WHERE user_id = $1
                     ORDER BY payment_date DESC",
                )
                .bind(user_id)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_user_charges")?;
                rows.into_iter().map(map_pg_charge).collect()
            }
        }
    }

    pub async fn get_all_charges(
        &self,
        plan_filter: Option<&str>,
        limit: Option<i64>,
        offset: i64,
    ) -> Result<Vec<Charge>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_all_charges connection")?;
                db::get_all_charges(&conn, plan_filter, limit, offset).context("sqlite get_all_charges")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, user_id, plan, telegram_charge_id, provider_charge_id, currency,
                            total_amount, invoice_payload, is_recurring, is_first_recurring,
                            CAST(subscription_expiration_date AS TEXT) AS subscription_expiration_date,
                            CAST(payment_date AS TEXT) AS payment_date,
                            CAST(created_at AS TEXT) AS created_at
                     FROM charges
                     WHERE ($1::text IS NULL OR plan = $1)
                     ORDER BY payment_date DESC
                     LIMIT $2 OFFSET $3",
                )
                .bind(plan_filter)
                .bind(limit.unwrap_or(-1))
                .bind(offset)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_all_charges")?;
                rows.into_iter().map(map_pg_charge).collect()
            }
        }
    }

    pub async fn get_charges_stats(&self) -> Result<(i64, i64, i64, i64, i64)> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_charges_stats connection")?;
                db::get_charges_stats(&conn).context("sqlite get_charges_stats")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT
                        COUNT(*)::bigint AS total_charges,
                        COALESCE(SUM(total_amount), 0)::bigint AS total_amount,
                        COALESCE(SUM(CASE WHEN plan = 'premium' THEN 1 ELSE 0 END), 0)::bigint AS premium_count,
                        COALESCE(SUM(CASE WHEN plan = 'vip' THEN 1 ELSE 0 END), 0)::bigint AS vip_count,
                        COALESCE(SUM(CASE WHEN is_recurring = 1 THEN 1 ELSE 0 END), 0)::bigint AS recurring_count
                     FROM charges",
                )
                .fetch_one(pg_pool)
                .await
                .context("postgres get_charges_stats")?;
                Ok((
                    row.get("total_charges"),
                    row.get("total_amount"),
                    row.get("premium_count"),
                    row.get("vip_count"),
                    row.get("recurring_count"),
                ))
            }
        }
    }

    pub async fn get_user_stats(&self, telegram_id: i64) -> Result<UserStats> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user_stats connection")?;
                db::get_user_stats(&conn, telegram_id).context("sqlite get_user_stats")
            }
            Self::Postgres { pg_pool, .. } => {
                let total_downloads =
                    sqlx::query_scalar::<_, i64>("SELECT COUNT(*)::bigint FROM download_history WHERE user_id = $1")
                        .bind(telegram_id)
                        .fetch_one(pg_pool)
                        .await
                        .context("postgres get_user_stats total_downloads")?;

                let total_size = sqlx::query_scalar::<_, Option<i64>>(
                    "SELECT SUM(
                        CASE
                            WHEN format = 'mp3' THEN 5000000
                            WHEN format = 'mp4' THEN 50000000
                            ELSE 1000000
                        END
                    )::bigint
                     FROM download_history
                     WHERE user_id = $1",
                )
                .bind(telegram_id)
                .fetch_one(pg_pool)
                .await
                .context("postgres get_user_stats total_size")?
                .unwrap_or(0);

                let active_days = sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(DISTINCT DATE(downloaded_at))::bigint
                     FROM download_history
                     WHERE user_id = $1",
                )
                .bind(telegram_id)
                .fetch_one(pg_pool)
                .await
                .context("postgres get_user_stats active_days")?;

                let title_rows = sqlx::query(
                    "SELECT title
                     FROM download_history
                     WHERE user_id = $1
                     ORDER BY downloaded_at DESC
                     LIMIT 100",
                )
                .bind(telegram_id)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_user_stats titles")?;

                let mut artist_counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
                for row in title_rows {
                    let title: String = row.get("title");
                    if let Some(pos) = title.find(" - ") {
                        let artist = title[..pos].trim().to_string();
                        if !artist.is_empty() {
                            *artist_counts.entry(artist).or_insert(0) += 1;
                        }
                    }
                }

                let mut top_artists: Vec<(String, i64)> = artist_counts.into_iter().collect();
                top_artists.sort_by(|a, b| b.1.cmp(&a.1));
                top_artists.truncate(5);

                let format_rows = sqlx::query(
                    "SELECT format, COUNT(*)::bigint AS cnt
                     FROM download_history
                     WHERE user_id = $1
                     GROUP BY format
                     ORDER BY cnt DESC
                     LIMIT 5",
                )
                .bind(telegram_id)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_user_stats top_formats")?;
                let top_formats = format_rows
                    .into_iter()
                    .map(|row| (row.get::<String, _>("format"), row.get::<i64, _>("cnt")))
                    .collect();

                let activity_rows = sqlx::query(
                    "SELECT DATE(downloaded_at)::text AS day, COUNT(*)::bigint AS cnt
                     FROM download_history
                     WHERE user_id = $1
                       AND downloaded_at >= NOW() - INTERVAL '7 days'
                     GROUP BY DATE(downloaded_at)
                     ORDER BY day DESC",
                )
                .bind(telegram_id)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_user_stats activity_by_day")?;
                let activity_by_day = activity_rows
                    .into_iter()
                    .map(|row| (row.get::<String, _>("day"), row.get::<i64, _>("cnt")))
                    .collect();

                Ok(UserStats {
                    total_downloads,
                    total_size,
                    active_days,
                    top_artists,
                    top_formats,
                    activity_by_day,
                })
            }
        }
    }

    pub async fn get_global_stats(&self) -> Result<GlobalStats> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_global_stats connection")?;
                db::get_global_stats(&conn).context("sqlite get_global_stats")
            }
            Self::Postgres { pg_pool, .. } => {
                let total_users =
                    sqlx::query_scalar::<_, i64>("SELECT COUNT(DISTINCT user_id)::bigint FROM download_history")
                        .fetch_one(pg_pool)
                        .await
                        .context("postgres get_global_stats total_users")?;

                let total_downloads = sqlx::query_scalar::<_, i64>("SELECT COUNT(*)::bigint FROM download_history")
                    .fetch_one(pg_pool)
                    .await
                    .context("postgres get_global_stats total_downloads")?;

                let top_track_rows = sqlx::query(
                    "SELECT title, COUNT(*)::bigint AS cnt
                     FROM download_history
                     GROUP BY title
                     ORDER BY cnt DESC
                     LIMIT 10",
                )
                .fetch_all(pg_pool)
                .await
                .context("postgres get_global_stats top_tracks")?;
                let top_tracks = top_track_rows
                    .into_iter()
                    .map(|row| (row.get::<String, _>("title"), row.get::<i64, _>("cnt")))
                    .collect();

                let top_format_rows = sqlx::query(
                    "SELECT format, COUNT(*)::bigint AS cnt
                     FROM download_history
                     GROUP BY format
                     ORDER BY cnt DESC",
                )
                .fetch_all(pg_pool)
                .await
                .context("postgres get_global_stats top_formats")?;
                let top_formats = top_format_rows
                    .into_iter()
                    .map(|row| (row.get::<String, _>("format"), row.get::<i64, _>("cnt")))
                    .collect();

                Ok(GlobalStats {
                    total_users,
                    total_downloads,
                    top_tracks,
                    top_formats,
                })
            }
        }
    }

    pub async fn update_subscription_data(
        &self,
        telegram_id: i64,
        plan: &str,
        charge_id: &str,
        subscription_expires_at: &str,
        is_recurring: bool,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite update_subscription_data connection")?;
                db::update_subscription_data(
                    &conn,
                    telegram_id,
                    plan,
                    charge_id,
                    subscription_expires_at,
                    is_recurring,
                )
                .context("sqlite update_subscription_data")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO subscriptions (user_id, plan, expires_at, telegram_charge_id, is_recurring, updated_at)
                     VALUES ($1, $2, $3, $4, $5, NOW())
                     ON CONFLICT (user_id) DO UPDATE SET
                        plan = EXCLUDED.plan,
                        expires_at = EXCLUDED.expires_at,
                        telegram_charge_id = EXCLUDED.telegram_charge_id,
                        is_recurring = EXCLUDED.is_recurring,
                        updated_at = NOW()",
                )
                .bind(telegram_id)
                .bind(plan)
                .bind(subscription_expires_at)
                .bind(charge_id)
                .bind(i32::from(is_recurring))
                .execute(pg_pool)
                .await
                .context("postgres update_subscription_data subscriptions")?;
                sqlx::query("UPDATE users SET plan = $2, updated_at = NOW() WHERE telegram_id = $1")
                    .bind(telegram_id)
                    .bind(plan)
                    .execute(pg_pool)
                    .await
                    .context("postgres update_subscription_data users")?;
                Ok(())
            }
        }
    }

    pub async fn cancel_subscription(&self, telegram_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite cancel_subscription connection")?;
                db::cancel_subscription(&conn, telegram_id).context("sqlite cancel_subscription")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO subscriptions (user_id, plan, is_recurring, updated_at)
                     VALUES ($1, 'free', 0, NOW())
                     ON CONFLICT (user_id) DO UPDATE SET
                        is_recurring = 0,
                        updated_at = NOW()",
                )
                .bind(telegram_id)
                .execute(pg_pool)
                .await
                .context("postgres cancel_subscription subscriptions")?;
                sqlx::query("UPDATE users SET plan = 'free', updated_at = NOW() WHERE telegram_id = $1")
                    .bind(telegram_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres cancel_subscription users")?;
                Ok(())
            }
        }
    }

    pub async fn create_user(&self, telegram_id: i64, username: Option<String>) -> Result<()> {
        self.create_user_with_language(telegram_id, username, None).await
    }

    pub async fn create_user_with_language(
        &self,
        telegram_id: i64,
        username: Option<String>,
        language: Option<&str>,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite create_user connection")?;
                if let Some(language) = language {
                    db::create_user_with_language(&conn, telegram_id, username, language)
                        .context("sqlite create_user_with_language")
                } else {
                    db::create_user(&conn, telegram_id, username).context("sqlite create_user")
                }
            }
            Self::Postgres { pg_pool, .. } => {
                let mut tx = pg_pool.begin().await.context("postgres create_user begin")?;
                sqlx::query(
                    "INSERT INTO users (
                        telegram_id, username, download_format, download_subtitles, video_quality,
                        audio_bitrate, language, send_as_document, send_audio_as_document
                     ) VALUES ($1, $2, 'mp3', 0, 'best', '320k', $3, 0, 0)
                     ON CONFLICT (telegram_id) DO NOTHING",
                )
                .bind(telegram_id)
                .bind(username)
                .bind(language.unwrap_or("en"))
                .execute(&mut *tx)
                .await
                .context("postgres create_user users insert")?;
                sqlx::query(
                    "INSERT INTO subscriptions (user_id, plan)
                     VALUES ($1, 'free')
                     ON CONFLICT (user_id) DO NOTHING",
                )
                .bind(telegram_id)
                .execute(&mut *tx)
                .await
                .context("postgres create_user subscriptions insert")?;
                tx.commit().await.context("postgres create_user commit")?;
                Ok(())
            }
        }
    }

    pub async fn log_request(&self, user_id: i64, request_text: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite log_request connection")?;
                db::log_request(&conn, user_id, request_text).context("sqlite log_request")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("INSERT INTO request_history (user_id, request_text) VALUES ($1, $2)")
                    .bind(user_id)
                    .bind(request_text)
                    .execute(pg_pool)
                    .await
                    .context("postgres log_request")?;
                Ok(())
            }
        }
    }

    pub async fn get_all_download_history(&self, telegram_id: i64) -> Result<Vec<DownloadHistoryEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_all_download_history connection")?;
                db::get_all_download_history(&conn, telegram_id).context("sqlite get_all_download_history")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, url, title, format, downloaded_at::text AS downloaded_at, file_id, author,
                            file_size, duration, video_quality, audio_bitrate, bot_api_url, bot_api_is_local,
                            source_id, part_index, category
                     FROM download_history
                     WHERE user_id = $1
                     ORDER BY downloaded_at DESC",
                )
                .bind(telegram_id)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_all_download_history")?;
                Ok(rows.into_iter().map(map_pg_download_history).collect())
            }
        }
    }

    pub async fn count_active_subscriptions(&self) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite count_active_subscriptions connection")?;
                let count = conn
                    .query_row(
                        "SELECT COUNT(*) FROM subscriptions WHERE expires_at > datetime('now')",
                        [],
                        |row| row.get(0),
                    )
                    .context("sqlite count_active_subscriptions")?;
                Ok(count)
            }
            Self::Postgres { pg_pool, .. } => {
                let count =
                    sqlx::query_scalar::<_, i64>("SELECT COUNT(*)::bigint FROM subscriptions WHERE expires_at > NOW()")
                        .fetch_one(pg_pool)
                        .await
                        .context("postgres count_active_subscriptions")?;
                Ok(count)
            }
        }
    }

    pub async fn count_daily_active_users(&self) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite count_daily_active_users connection")?;
                let count = conn
                    .query_row(
                        "SELECT COUNT(DISTINCT user_id) FROM request_history WHERE date(timestamp) = date('now')",
                        [],
                        |row| row.get(0),
                    )
                    .context("sqlite count_daily_active_users")?;
                Ok(count)
            }
            Self::Postgres { pg_pool, .. } => {
                let count = sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(DISTINCT user_id)::bigint FROM request_history WHERE DATE(timestamp) = CURRENT_DATE",
                )
                .fetch_one(pg_pool)
                .await
                .context("postgres count_daily_active_users")?;
                Ok(count)
            }
        }
    }

    pub async fn count_monthly_active_users(&self) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite count_monthly_active_users connection")?;
                let count = conn
                    .query_row(
                        "SELECT COUNT(DISTINCT user_id) FROM request_history WHERE timestamp >= datetime('now', '-30 days')",
                        [],
                        |row| row.get(0),
                    )
                    .context("sqlite count_monthly_active_users")?;
                Ok(count)
            }
            Self::Postgres { pg_pool, .. } => {
                let count = sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(DISTINCT user_id)::bigint FROM request_history WHERE timestamp >= NOW() - INTERVAL '30 days'",
                )
                .fetch_one(pg_pool)
                .await
                .context("postgres count_monthly_active_users")?;
                Ok(count)
            }
        }
    }

    pub async fn log_alert_history(
        &self,
        alert_type: &str,
        severity: &str,
        message: &str,
        metadata: Option<&str>,
        triggered_at: &str,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite log_alert_history connection")?;
                conn.execute(
                    "INSERT INTO alert_history (alert_type, severity, message, metadata, triggered_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![alert_type, severity, message, metadata, triggered_at],
                )
                .context("sqlite log_alert_history")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO alert_history (alert_type, severity, message, metadata, triggered_at)
                     VALUES ($1, $2, $3, $4, $5::timestamptz)",
                )
                .bind(alert_type)
                .bind(severity)
                .bind(message)
                .bind(metadata)
                .bind(triggered_at)
                .execute(pg_pool)
                .await
                .context("postgres log_alert_history")?;
                Ok(())
            }
        }
    }

    pub async fn resolve_alert_history(&self, alert_type: &str, resolved_at: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite resolve_alert_history connection")?;
                conn.execute(
                    "UPDATE alert_history SET resolved_at = ?1 WHERE alert_type = ?2 AND resolved_at IS NULL",
                    rusqlite::params![resolved_at, alert_type],
                )
                .context("sqlite resolve_alert_history")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE alert_history SET resolved_at = $1::timestamptz
                     WHERE alert_type = $2 AND resolved_at IS NULL",
                )
                .bind(resolved_at)
                .bind(alert_type)
                .execute(pg_pool)
                .await
                .context("postgres resolve_alert_history")?;
                Ok(())
            }
        }
    }

    pub async fn log_error(
        &self,
        user_id: Option<i64>,
        username: Option<&str>,
        error_type: &str,
        error_message: &str,
        url: Option<&str>,
        context: Option<&str>,
    ) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite log_error connection")?;
                db::log_error(&conn, user_id, username, error_type, error_message, url, context)
                    .context("sqlite log_error")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "INSERT INTO error_log (user_id, username, error_type, error_message, url, context)
                     VALUES ($1, $2, $3, $4, $5, $6)
                     RETURNING id",
                )
                .bind(user_id)
                .bind(username)
                .bind(error_type)
                .bind(error_message)
                .bind(url)
                .bind(context)
                .fetch_one(pg_pool)
                .await
                .context("postgres log_error")?;
                Ok(row.get("id"))
            }
        }
    }

    pub async fn get_recent_errors(&self, hours: i64, limit: i64) -> Result<Vec<ErrorLogEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_recent_errors connection")?;
                db::get_recent_errors(&conn, hours, limit).context("sqlite get_recent_errors")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, timestamp::text AS timestamp, user_id, username, error_type, error_message, url, context, resolved
                     FROM error_log
                     WHERE timestamp >= NOW() - make_interval(hours => $1)
                     ORDER BY timestamp DESC
                     LIMIT $2",
                )
                .bind(hours as i32)
                .bind(limit)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_recent_errors")?;
                Ok(rows.into_iter().map(map_pg_error_log_entry).collect())
            }
        }
    }

    pub async fn get_error_stats(&self, hours: i64) -> Result<Vec<(String, i64)>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_error_stats connection")?;
                db::get_error_stats(&conn, hours).context("sqlite get_error_stats")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT error_type, COUNT(*)::bigint AS cnt
                     FROM error_log
                     WHERE timestamp >= NOW() - make_interval(hours => $1)
                     GROUP BY error_type
                     ORDER BY cnt DESC",
                )
                .bind(hours as i32)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_error_stats")?;
                Ok(rows
                    .into_iter()
                    .map(|row| (row.get::<String, _>("error_type"), row.get::<i64, _>("cnt")))
                    .collect())
            }
        }
    }

    pub async fn cleanup_old_errors(&self, days: i64) -> Result<usize> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite cleanup_old_errors connection")?;
                db::cleanup_old_errors(&conn, days).context("sqlite cleanup_old_errors")
            }
            Self::Postgres { pg_pool, .. } => {
                let result = sqlx::query("DELETE FROM error_log WHERE timestamp < NOW() - make_interval(days => $1)")
                    .bind(days as i32)
                    .execute(pg_pool)
                    .await
                    .context("postgres cleanup_old_errors")?;
                Ok(result.rows_affected() as usize)
            }
        }
    }

    pub async fn cleanup_old_tasks(&self, days: i64) -> Result<usize> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite cleanup_old_tasks connection")?;
                db::cleanup_old_tasks(&conn, days).context("sqlite cleanup_old_tasks")
            }
            Self::Postgres { pg_pool, .. } => {
                let result = sqlx::query(
                    "DELETE FROM task_queue
                     WHERE status IN ('completed', 'dead_letter')
                       AND updated_at < NOW() - make_interval(days => $1)",
                )
                .bind(days as i32)
                .execute(pg_pool)
                .await
                .context("postgres cleanup_old_tasks")?;
                Ok(result.rows_affected() as usize)
            }
        }
    }

    pub async fn create_share_page_record(
        &self,
        id: &str,
        youtube_url: &str,
        title: &str,
        artist: Option<&str>,
        thumbnail_url: Option<&str>,
        duration_secs: Option<i64>,
        streaming_links_json: Option<&str>,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite create_share_page_record connection")?;
                conn.execute(
                    "INSERT INTO share_pages (id, youtube_url, title, artist, thumbnail_url, duration_secs, streaming_links)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        id,
                        youtube_url,
                        title,
                        artist,
                        thumbnail_url,
                        duration_secs,
                        streaming_links_json
                    ],
                )
                .context("sqlite create_share_page_record")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO share_pages (id, youtube_url, title, artist, thumbnail_url, duration_secs, streaming_links)
                     VALUES ($1, $2, $3, $4, $5, $6, $7)",
                )
                .bind(id)
                .bind(youtube_url)
                .bind(title)
                .bind(artist)
                .bind(thumbnail_url)
                .bind(duration_secs)
                .bind(streaming_links_json)
                .execute(pg_pool)
                .await
                .context("postgres create_share_page_record")?;
                Ok(())
            }
        }
    }

    pub async fn get_share_page_record(&self, id: &str) -> Result<Option<SharePageRecord>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_share_page_record connection")?;
                let result = conn
                    .query_row(
                        "SELECT id, youtube_url, title, artist, thumbnail_url, duration_secs, streaming_links, created_at
                         FROM share_pages WHERE id = ?1",
                        rusqlite::params![id],
                        |row| {
                            Ok(SharePageRecord {
                                id: row.get(0)?,
                                youtube_url: row.get(1)?,
                                title: row.get(2)?,
                                artist: row.get(3)?,
                                thumbnail_url: row.get(4)?,
                                duration_secs: row.get(5)?,
                                streaming_links_json: row.get(6)?,
                                created_at: row.get(7)?,
                            })
                        },
                    )
                    .optional()
                    .context("sqlite get_share_page_record")?;
                Ok(result)
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT id, youtube_url, title, artist, thumbnail_url, duration_secs,
                            streaming_links, created_at::text AS created_at
                     FROM share_pages WHERE id = $1",
                )
                .bind(id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_share_page_record")?;
                Ok(row.map(map_pg_share_page_record))
            }
        }
    }

    pub async fn store_cached_url(&self, id: &str, url: &str, expires_at: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite store_cached_url connection")?;
                conn.execute(
                    "INSERT OR REPLACE INTO url_cache (id, url, expires_at) VALUES (?1, ?2, ?3)",
                    rusqlite::params![id, url, expires_at],
                )
                .context("sqlite store_cached_url")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO url_cache (id, url, expires_at)
                     VALUES ($1, $2, $3::timestamptz)
                     ON CONFLICT (id) DO UPDATE
                     SET url = EXCLUDED.url,
                         expires_at = EXCLUDED.expires_at",
                )
                .bind(id)
                .bind(url)
                .bind(expires_at)
                .execute(pg_pool)
                .await
                .context("postgres store_cached_url")?;
                Ok(())
            }
        }
    }

    pub async fn get_cached_url(&self, id: &str) -> Result<Option<String>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_cached_url connection")?;
                let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
                conn.query_row(
                    "SELECT url FROM url_cache WHERE id = ?1 AND expires_at > ?2",
                    rusqlite::params![id, now],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .context("sqlite get_cached_url")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query("SELECT url FROM url_cache WHERE id = $1 AND expires_at > NOW()")
                    .bind(id)
                    .fetch_optional(pg_pool)
                    .await
                    .context("postgres get_cached_url")?;
                Ok(row.map(|row| row.get("url")))
            }
        }
    }

    pub async fn cleanup_expired_url_cache(&self) -> Result<usize> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite cleanup_expired_url_cache connection")?;
                let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
                conn.execute("DELETE FROM url_cache WHERE expires_at <= ?1", rusqlite::params![now])
                    .context("sqlite cleanup_expired_url_cache")
            }
            Self::Postgres { pg_pool, .. } => {
                let result = sqlx::query("DELETE FROM url_cache WHERE expires_at <= NOW()")
                    .execute(pg_pool)
                    .await
                    .context("postgres cleanup_expired_url_cache")?;
                Ok(result.rows_affected() as usize)
            }
        }
    }

    pub async fn get_user_language(&self, telegram_id: i64) -> Result<String> {
        self.get_user_string_setting(
            telegram_id,
            "language",
            "SELECT language FROM users WHERE telegram_id = $1",
            "ru",
        )
        .await
    }

    pub async fn get_user_progress_bar_style(&self, telegram_id: i64) -> Result<String> {
        self.get_user_string_setting(
            telegram_id,
            "progress_bar_style",
            "SELECT progress_bar_style FROM users WHERE telegram_id = $1",
            "classic",
        )
        .await
    }

    pub async fn get_user_video_quality(&self, telegram_id: i64) -> Result<String> {
        self.get_user_string_setting(
            telegram_id,
            "video_quality",
            "SELECT video_quality FROM users WHERE telegram_id = $1",
            "best",
        )
        .await
    }

    pub async fn get_user_download_format(&self, telegram_id: i64) -> Result<String> {
        self.get_user_string_setting(
            telegram_id,
            "download_format",
            "SELECT download_format FROM users WHERE telegram_id = $1",
            "mp3",
        )
        .await
    }

    pub async fn get_user_audio_bitrate(&self, telegram_id: i64) -> Result<String> {
        self.get_user_string_setting(
            telegram_id,
            "audio_bitrate",
            "SELECT audio_bitrate FROM users WHERE telegram_id = $1",
            "320k",
        )
        .await
    }

    pub async fn get_user_send_as_document(&self, telegram_id: i64) -> Result<i32> {
        self.get_user_i32_setting(
            telegram_id,
            "send_as_document",
            "SELECT send_as_document FROM users WHERE telegram_id = $1",
            0,
        )
        .await
    }

    pub async fn get_user_send_audio_as_document(&self, telegram_id: i64) -> Result<i32> {
        self.get_user_i32_setting(
            telegram_id,
            "send_audio_as_document",
            "SELECT send_audio_as_document FROM users WHERE telegram_id = $1",
            0,
        )
        .await
    }

    pub async fn get_user_download_subtitles(&self, telegram_id: i64) -> Result<bool> {
        Ok(self
            .get_user_i32_setting(
                telegram_id,
                "download_subtitles",
                "SELECT download_subtitles FROM users WHERE telegram_id = $1",
                0,
            )
            .await?
            == 1)
    }

    pub async fn get_user_burn_subtitles(&self, telegram_id: i64) -> Result<bool> {
        Ok(self
            .get_user_i32_setting(
                telegram_id,
                "burn_subtitles",
                "SELECT COALESCE(burn_subtitles, 0) FROM users WHERE telegram_id = $1",
                0,
            )
            .await?
            == 1)
    }

    pub async fn get_user_subtitle_style(&self, telegram_id: i64) -> Result<SubtitleStyle> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user_subtitle_style connection")?;
                db::get_user_subtitle_style(&conn, telegram_id).context("sqlite get_user_subtitle_style")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT
                        COALESCE(subtitle_font_size, 'medium') AS subtitle_font_size,
                        COALESCE(subtitle_text_color, 'white') AS subtitle_text_color,
                        COALESCE(subtitle_outline_color, 'black') AS subtitle_outline_color,
                        COALESCE(subtitle_outline_width, 2) AS subtitle_outline_width,
                        COALESCE(subtitle_shadow, 1) AS subtitle_shadow,
                        COALESCE(subtitle_position, 'bottom') AS subtitle_position
                     FROM users
                     WHERE telegram_id = $1",
                )
                .bind(telegram_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_user_subtitle_style")?;
                Ok(row.map(map_pg_subtitle_style).unwrap_or_default())
            }
        }
    }

    pub async fn set_user_video_quality(&self, telegram_id: i64, quality: &str) -> Result<()> {
        self.set_user_string_setting(
            telegram_id,
            "video_quality",
            quality,
            "UPDATE users SET video_quality = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_download_format(&self, telegram_id: i64, format: &str) -> Result<()> {
        self.set_user_string_setting(
            telegram_id,
            "download_format",
            format,
            "UPDATE users SET download_format = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_audio_bitrate(&self, telegram_id: i64, bitrate: &str) -> Result<()> {
        self.set_user_string_setting(
            telegram_id,
            "audio_bitrate",
            bitrate,
            "UPDATE users SET audio_bitrate = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_send_as_document(&self, telegram_id: i64, send_as_document: i32) -> Result<()> {
        self.set_user_i32_setting(
            telegram_id,
            "send_as_document",
            send_as_document,
            "UPDATE users SET send_as_document = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_send_audio_as_document(&self, telegram_id: i64, send_audio_as_document: i32) -> Result<()> {
        self.set_user_i32_setting(
            telegram_id,
            "send_audio_as_document",
            send_audio_as_document,
            "UPDATE users SET send_audio_as_document = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_burn_subtitles(&self, telegram_id: i64, enabled: bool) -> Result<()> {
        self.set_user_i32_setting(
            telegram_id,
            "burn_subtitles",
            i32::from(enabled),
            "UPDATE users SET burn_subtitles = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_language(&self, telegram_id: i64, language: &str) -> Result<()> {
        self.set_user_string_setting(
            telegram_id,
            "language",
            language,
            "UPDATE users SET language = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_subtitle_font_size(&self, telegram_id: i64, value: &str) -> Result<()> {
        self.set_user_string_setting(
            telegram_id,
            "subtitle_font_size",
            value,
            "UPDATE users SET subtitle_font_size = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_subtitle_text_color(&self, telegram_id: i64, value: &str) -> Result<()> {
        self.set_user_string_setting(
            telegram_id,
            "subtitle_text_color",
            value,
            "UPDATE users SET subtitle_text_color = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_subtitle_outline_color(&self, telegram_id: i64, value: &str) -> Result<()> {
        self.set_user_string_setting(
            telegram_id,
            "subtitle_outline_color",
            value,
            "UPDATE users SET subtitle_outline_color = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_subtitle_outline_width(&self, telegram_id: i64, value: i32) -> Result<()> {
        self.set_user_i32_setting(
            telegram_id,
            "subtitle_outline_width",
            value,
            "UPDATE users SET subtitle_outline_width = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_subtitle_shadow(&self, telegram_id: i64, value: i32) -> Result<()> {
        self.set_user_i32_setting(
            telegram_id,
            "subtitle_shadow",
            value,
            "UPDATE users SET subtitle_shadow = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_subtitle_position(&self, telegram_id: i64, value: &str) -> Result<()> {
        self.set_user_string_setting(
            telegram_id,
            "subtitle_position",
            value,
            "UPDATE users SET subtitle_position = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_progress_bar_style(&self, telegram_id: i64, style: &str) -> Result<()> {
        self.set_user_string_setting(
            telegram_id,
            "progress_bar_style",
            style,
            "UPDATE users SET progress_bar_style = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn get_bot_asset(&self, key: &str) -> Result<Option<String>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_bot_asset connection")?;
                db::get_bot_asset(&conn, key).context("sqlite get_bot_asset")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query("SELECT file_id FROM bot_assets WHERE key = $1")
                    .bind(key)
                    .fetch_optional(pg_pool)
                    .await
                    .context("postgres get_bot_asset")?;
                Ok(row.map(|row| row.get("file_id")))
            }
        }
    }

    pub async fn set_bot_asset(&self, key: &str, file_id: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite set_bot_asset connection")?;
                db::set_bot_asset(&conn, key, file_id).context("sqlite set_bot_asset")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO bot_assets (key, file_id, created_at)
                     VALUES ($1, $2, NOW())
                     ON CONFLICT (key) DO UPDATE SET file_id = EXCLUDED.file_id, created_at = NOW()",
                )
                .bind(key)
                .bind(file_id)
                .execute(pg_pool)
                .await
                .context("postgres set_bot_asset")?;
                Ok(())
            }
        }
    }

    pub async fn get_user_content_subscriptions(&self, user_id: i64) -> Result<Vec<ContentSubscriptionRecord>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user_content_subscriptions connection")?;
                sqlite_get_user_content_subscriptions(&conn, user_id)
                    .map_err(anyhow::Error::msg)
                    .context("sqlite get_user_content_subscriptions")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, user_id, source_type, source_id, display_name, watch_mask, is_active,
                            last_seen_state, source_meta, CAST(last_checked_at AS TEXT) AS last_checked_at,
                            last_error, consecutive_errors, CAST(created_at AS TEXT) AS created_at,
                            CAST(updated_at AS TEXT) AS updated_at
                     FROM content_subscriptions
                     WHERE user_id = $1 AND is_active = 1
                     ORDER BY created_at ASC",
                )
                .bind(user_id)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_user_content_subscriptions")?;
                rows.into_iter().map(map_pg_content_subscription).collect()
            }
        }
    }

    pub async fn count_user_content_subscriptions(&self, user_id: i64) -> Result<u32> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite count_user_content_subscriptions connection")?;
                sqlite_count_user_content_subscriptions(&conn, user_id)
                    .map_err(anyhow::Error::msg)
                    .context("sqlite count_user_content_subscriptions")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT COUNT(*)::BIGINT AS count
                     FROM content_subscriptions
                     WHERE user_id = $1 AND is_active = 1",
                )
                .bind(user_id)
                .fetch_one(pg_pool)
                .await
                .context("postgres count_user_content_subscriptions")?;
                Ok(row.get::<i64, _>("count") as u32)
            }
        }
    }

    pub async fn get_content_subscription(&self, id: i64) -> Result<Option<ContentSubscriptionRecord>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_content_subscription connection")?;
                sqlite_get_content_subscription(&conn, id)
                    .map_err(anyhow::Error::msg)
                    .context("sqlite get_content_subscription")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT id, user_id, source_type, source_id, display_name, watch_mask, is_active,
                            last_seen_state, source_meta, CAST(last_checked_at AS TEXT) AS last_checked_at,
                            last_error, consecutive_errors, CAST(created_at AS TEXT) AS created_at,
                            CAST(updated_at AS TEXT) AS updated_at
                     FROM content_subscriptions
                     WHERE id = $1",
                )
                .bind(id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_content_subscription")?;
                row.map(map_pg_content_subscription).transpose()
            }
        }
    }

    pub async fn has_content_subscription(
        &self,
        user_id: i64,
        source_type: &str,
        source_id: &str,
    ) -> Result<Option<ContentSubscriptionRecord>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite has_content_subscription connection")?;
                sqlite_has_content_subscription(&conn, user_id, source_type, source_id)
                    .map_err(anyhow::Error::msg)
                    .context("sqlite has_content_subscription")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT id, user_id, source_type, source_id, display_name, watch_mask, is_active,
                            last_seen_state, source_meta, CAST(last_checked_at AS TEXT) AS last_checked_at,
                            last_error, consecutive_errors, CAST(created_at AS TEXT) AS created_at,
                            CAST(updated_at AS TEXT) AS updated_at
                     FROM content_subscriptions
                     WHERE user_id = $1 AND source_type = $2 AND source_id = $3",
                )
                .bind(user_id)
                .bind(source_type)
                .bind(source_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres has_content_subscription")?;
                row.map(map_pg_content_subscription).transpose()
            }
        }
    }

    pub async fn upsert_content_subscription(
        &self,
        user_id: i64,
        source_type: &str,
        source_id: &str,
        display_name: &str,
        watch_mask: u32,
        source_meta: Option<&JsonValue>,
    ) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite upsert_content_subscription connection")?;
                sqlite_upsert_content_subscription(
                    &conn,
                    user_id,
                    source_type,
                    source_id,
                    display_name,
                    watch_mask,
                    source_meta,
                )
                .map_err(anyhow::Error::msg)
                .context("sqlite upsert_content_subscription")
            }
            Self::Postgres { pg_pool, .. } => {
                let meta_json = source_meta.map(|value| value.to_string());
                let row = sqlx::query(
                    "INSERT INTO content_subscriptions (
                        user_id, source_type, source_id, display_name, watch_mask, source_meta,
                        is_active, consecutive_errors, updated_at
                     ) VALUES ($1, $2, $3, $4, $5, $6, 1, 0, NOW())
                     ON CONFLICT (user_id, source_type, source_id) DO UPDATE SET
                        watch_mask = EXCLUDED.watch_mask,
                        display_name = EXCLUDED.display_name,
                        source_meta = COALESCE(EXCLUDED.source_meta, content_subscriptions.source_meta),
                        is_active = 1,
                        consecutive_errors = 0,
                        last_error = NULL,
                        updated_at = NOW()
                     RETURNING id",
                )
                .bind(user_id)
                .bind(source_type)
                .bind(source_id)
                .bind(display_name)
                .bind(watch_mask as i32)
                .bind(meta_json)
                .fetch_one(pg_pool)
                .await
                .context("postgres upsert_content_subscription")?;
                Ok(row.get("id"))
            }
        }
    }

    pub async fn deactivate_content_subscription(&self, id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite deactivate_content_subscription connection")?;
                sqlite_deactivate_content_subscription(&conn, id)
                    .map_err(anyhow::Error::msg)
                    .context("sqlite deactivate_content_subscription")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE content_subscriptions
                     SET is_active = 0, updated_at = NOW()
                     WHERE id = $1",
                )
                .bind(id)
                .execute(pg_pool)
                .await
                .context("postgres deactivate_content_subscription")?;
                Ok(())
            }
        }
    }

    pub async fn deactivate_all_content_subscriptions_for_user(&self, user_id: i64) -> Result<u32> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool)
                    .context("sqlite deactivate_all_content_subscriptions_for_user connection")?;
                sqlite_deactivate_all_content_subscriptions_for_user(&conn, user_id)
                    .map_err(anyhow::Error::msg)
                    .context("sqlite deactivate_all_content_subscriptions_for_user")
            }
            Self::Postgres { pg_pool, .. } => {
                let result = sqlx::query(
                    "UPDATE content_subscriptions
                     SET is_active = 0, updated_at = NOW()
                     WHERE user_id = $1 AND is_active = 1",
                )
                .bind(user_id)
                .execute(pg_pool)
                .await
                .context("postgres deactivate_all_content_subscriptions_for_user")?;
                Ok(result.rows_affected() as u32)
            }
        }
    }

    pub async fn update_content_watch_mask(&self, id: i64, new_mask: u32) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite update_content_watch_mask connection")?;
                sqlite_update_content_watch_mask(&conn, id, new_mask)
                    .map_err(anyhow::Error::msg)
                    .context("sqlite update_content_watch_mask")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE content_subscriptions
                     SET watch_mask = $1, updated_at = NOW()
                     WHERE id = $2",
                )
                .bind(new_mask as i32)
                .bind(id)
                .execute(pg_pool)
                .await
                .context("postgres update_content_watch_mask")?;
                Ok(())
            }
        }
    }

    pub async fn get_active_content_source_groups(&self) -> Result<Vec<ContentSourceGroup>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_active_content_source_groups connection")?;
                sqlite_get_active_content_source_groups(&conn)
                    .map_err(anyhow::Error::msg)
                    .context("sqlite get_active_content_source_groups")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, user_id, source_type, source_id, display_name, watch_mask, is_active,
                            last_seen_state, source_meta, CAST(last_checked_at AS TEXT) AS last_checked_at,
                            last_error, consecutive_errors, CAST(created_at AS TEXT) AS created_at,
                            CAST(updated_at AS TEXT) AS updated_at
                     FROM content_subscriptions
                     WHERE is_active = 1
                     ORDER BY last_checked_at ASC NULLS FIRST, source_type, source_id",
                )
                .fetch_all(pg_pool)
                .await
                .context("postgres get_active_content_source_groups")?;
                let all_subs: Vec<ContentSubscriptionRecord> = rows
                    .into_iter()
                    .map(map_pg_content_subscription)
                    .collect::<Result<_>>()?;
                Ok(group_content_subscriptions(all_subs))
            }
        }
    }

    pub async fn update_content_check_success(
        &self,
        source_type: &str,
        source_id: &str,
        new_state: &JsonValue,
        new_meta: Option<&JsonValue>,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite update_content_check_success connection")?;
                sqlite_update_content_check_success(&conn, source_type, source_id, new_state, new_meta)
                    .map_err(anyhow::Error::msg)
                    .context("sqlite update_content_check_success")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE content_subscriptions
                     SET last_seen_state = $1,
                         source_meta = COALESCE($2, source_meta),
                         last_checked_at = NOW(),
                         last_error = NULL,
                         consecutive_errors = 0,
                         updated_at = NOW()
                     WHERE source_type = $3 AND source_id = $4 AND is_active = 1",
                )
                .bind(new_state.to_string())
                .bind(new_meta.map(|value| value.to_string()))
                .bind(source_type)
                .bind(source_id)
                .execute(pg_pool)
                .await
                .context("postgres update_content_check_success")?;
                Ok(())
            }
        }
    }

    pub async fn update_content_check_error(&self, source_type: &str, source_id: &str, error: &str) -> Result<u32> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite update_content_check_error connection")?;
                sqlite_update_content_check_error(&conn, source_type, source_id, error)
                    .map_err(anyhow::Error::msg)
                    .context("sqlite update_content_check_error")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE content_subscriptions
                     SET last_checked_at = NOW(),
                         last_error = $1,
                         consecutive_errors = consecutive_errors + 1,
                         updated_at = NOW()
                     WHERE source_type = $2 AND source_id = $3 AND is_active = 1",
                )
                .bind(error)
                .bind(source_type)
                .bind(source_id)
                .execute(pg_pool)
                .await
                .context("postgres update_content_check_error update")?;
                let row = sqlx::query(
                    "SELECT COALESCE(MAX(consecutive_errors), 0)::BIGINT AS max_errors
                     FROM content_subscriptions
                     WHERE source_type = $1 AND source_id = $2 AND is_active = 1",
                )
                .bind(source_type)
                .bind(source_id)
                .fetch_one(pg_pool)
                .await
                .context("postgres update_content_check_error select")?;
                Ok(row.get::<i64, _>("max_errors") as u32)
            }
        }
    }

    pub async fn auto_disable_errored_content(
        &self,
        source_type: &str,
        source_id: &str,
        max_errors: u32,
    ) -> Result<u32> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite auto_disable_errored_content connection")?;
                sqlite_auto_disable_errored_content(&conn, source_type, source_id, max_errors)
                    .map_err(anyhow::Error::msg)
                    .context("sqlite auto_disable_errored_content")
            }
            Self::Postgres { pg_pool, .. } => {
                let result = sqlx::query(
                    "UPDATE content_subscriptions
                     SET is_active = 0, updated_at = NOW()
                     WHERE source_type = $1 AND source_id = $2
                       AND is_active = 1 AND consecutive_errors >= $3",
                )
                .bind(source_type)
                .bind(source_id)
                .bind(max_errors as i32)
                .execute(pg_pool)
                .await
                .context("postgres auto_disable_errored_content")?;
                Ok(result.rows_affected() as u32)
            }
        }
    }

    pub async fn create_playlist(&self, user_id: i64, name: &str, description: Option<&str>) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite create_playlist connection")?;
                db::create_playlist(&conn, user_id, name, description).context("sqlite create_playlist")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "INSERT INTO playlists (user_id, name, description, created_at, updated_at)
                     VALUES ($1, $2, $3, NOW(), NOW())
                     RETURNING id",
                )
                .bind(user_id)
                .bind(name)
                .bind(description)
                .fetch_one(pg_pool)
                .await
                .context("postgres create_playlist")?;
                Ok(row.get("id"))
            }
        }
    }

    pub async fn get_playlist(&self, playlist_id: i64) -> Result<Option<Playlist>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_playlist connection")?;
                db::get_playlist(&conn, playlist_id).context("sqlite get_playlist")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT id, user_id, name, description, is_public, share_token,
                            CAST(created_at AS TEXT) AS created_at,
                            CAST(updated_at AS TEXT) AS updated_at
                     FROM playlists
                     WHERE id = $1",
                )
                .bind(playlist_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_playlist")?;
                row.map(map_pg_playlist).transpose()
            }
        }
    }

    pub async fn get_user_playlists(&self, user_id: i64) -> Result<Vec<Playlist>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user_playlists connection")?;
                db::get_user_playlists(&conn, user_id).context("sqlite get_user_playlists")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, user_id, name, description, is_public, share_token,
                            CAST(created_at AS TEXT) AS created_at,
                            CAST(updated_at AS TEXT) AS updated_at
                     FROM playlists
                     WHERE user_id = $1
                     ORDER BY updated_at DESC",
                )
                .bind(user_id)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_user_playlists")?;
                rows.into_iter().map(map_pg_playlist).collect()
            }
        }
    }

    pub async fn rename_playlist(&self, playlist_id: i64, name: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite rename_playlist connection")?;
                db::rename_playlist(&conn, playlist_id, name).context("sqlite rename_playlist")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE playlists
                     SET name = $2, updated_at = NOW()
                     WHERE id = $1",
                )
                .bind(playlist_id)
                .bind(name)
                .execute(pg_pool)
                .await
                .context("postgres rename_playlist")?;
                Ok(())
            }
        }
    }

    pub async fn delete_playlist(&self, playlist_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite delete_playlist connection")?;
                db::delete_playlist(&conn, playlist_id).context("sqlite delete_playlist")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("DELETE FROM playlists WHERE id = $1")
                    .bind(playlist_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres delete_playlist")?;
                Ok(())
            }
        }
    }

    pub async fn count_user_playlists(&self, user_id: i64) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite count_user_playlists connection")?;
                db::count_user_playlists(&conn, user_id).context("sqlite count_user_playlists")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT COUNT(*)::BIGINT AS count
                     FROM playlists
                     WHERE user_id = $1",
                )
                .bind(user_id)
                .fetch_one(pg_pool)
                .await
                .context("postgres count_user_playlists")?;
                Ok(row.get("count"))
            }
        }
    }

    pub async fn set_playlist_share_token(&self, playlist_id: i64, token: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite set_playlist_share_token connection")?;
                db::set_playlist_share_token(&conn, playlist_id, token).context("sqlite set_playlist_share_token")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE playlists
                     SET share_token = $2, updated_at = NOW()
                     WHERE id = $1",
                )
                .bind(playlist_id)
                .bind(token)
                .execute(pg_pool)
                .await
                .context("postgres set_playlist_share_token")?;
                Ok(())
            }
        }
    }

    pub async fn set_playlist_public(&self, playlist_id: i64, is_public: bool) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite set_playlist_public connection")?;
                db::set_playlist_public(&conn, playlist_id, is_public).context("sqlite set_playlist_public")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE playlists
                     SET is_public = $2, updated_at = NOW()
                     WHERE id = $1",
                )
                .bind(playlist_id)
                .bind(if is_public { 1_i32 } else { 0_i32 })
                .execute(pg_pool)
                .await
                .context("postgres set_playlist_public")?;
                Ok(())
            }
        }
    }

    pub async fn get_playlist_by_share_token(&self, token: &str) -> Result<Option<Playlist>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_playlist_by_share_token connection")?;
                db::get_playlist_by_share_token(&conn, token).context("sqlite get_playlist_by_share_token")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT id, user_id, name, description, is_public, share_token,
                            CAST(created_at AS TEXT) AS created_at,
                            CAST(updated_at AS TEXT) AS updated_at
                     FROM playlists
                     WHERE share_token = $1",
                )
                .bind(token)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_playlist_by_share_token")?;
                row.map(map_pg_playlist).transpose()
            }
        }
    }

    pub async fn add_playlist_item(
        &self,
        playlist_id: i64,
        title: &str,
        artist: Option<&str>,
        url: &str,
        duration_secs: Option<i32>,
        file_id: Option<&str>,
        source: &str,
    ) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite add_playlist_item connection")?;
                db::add_playlist_item(&conn, playlist_id, title, artist, url, duration_secs, file_id, source)
                    .context("sqlite add_playlist_item")
            }
            Self::Postgres { pg_pool, .. } => {
                let mut tx = pg_pool.begin().await.context("postgres add_playlist_item begin")?;
                let row = sqlx::query(
                    "SELECT COALESCE(MAX(position), -1) + 1 AS next_position
                     FROM playlist_items
                     WHERE playlist_id = $1",
                )
                .bind(playlist_id)
                .fetch_one(&mut *tx)
                .await
                .context("postgres add_playlist_item next_position")?;
                let next_position: i32 = row.get("next_position");
                let inserted = sqlx::query(
                    "INSERT INTO playlist_items (
                        playlist_id, position, title, artist, url, duration_secs, file_id, source, added_at
                     ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW())
                     RETURNING id",
                )
                .bind(playlist_id)
                .bind(next_position)
                .bind(title)
                .bind(artist)
                .bind(url)
                .bind(duration_secs)
                .bind(file_id)
                .bind(source)
                .fetch_one(&mut *tx)
                .await
                .context("postgres add_playlist_item insert")?;
                sqlx::query("UPDATE playlists SET updated_at = NOW() WHERE id = $1")
                    .bind(playlist_id)
                    .execute(&mut *tx)
                    .await
                    .context("postgres add_playlist_item touch_playlist")?;
                tx.commit().await.context("postgres add_playlist_item commit")?;
                Ok(inserted.get("id"))
            }
        }
    }

    pub async fn remove_playlist_item(&self, item_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite remove_playlist_item connection")?;
                db::remove_playlist_item(&conn, item_id).context("sqlite remove_playlist_item")
            }
            Self::Postgres { pg_pool, .. } => {
                let mut tx = pg_pool.begin().await.context("postgres remove_playlist_item begin")?;
                let playlist_id = sqlx::query("SELECT playlist_id FROM playlist_items WHERE id = $1")
                    .bind(item_id)
                    .fetch_optional(&mut *tx)
                    .await
                    .context("postgres remove_playlist_item select")?
                    .map(|row| row.get::<i64, _>("playlist_id"));
                sqlx::query("DELETE FROM playlist_items WHERE id = $1")
                    .bind(item_id)
                    .execute(&mut *tx)
                    .await
                    .context("postgres remove_playlist_item delete")?;
                if let Some(playlist_id) = playlist_id {
                    sqlx::query("UPDATE playlists SET updated_at = NOW() WHERE id = $1")
                        .bind(playlist_id)
                        .execute(&mut *tx)
                        .await
                        .context("postgres remove_playlist_item touch_playlist")?;
                }
                tx.commit().await.context("postgres remove_playlist_item commit")?;
                Ok(())
            }
        }
    }

    pub async fn reorder_playlist_item(&self, item_id: i64, direction: i32) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite reorder_playlist_item connection")?;
                db::reorder_playlist_item(&conn, item_id, direction).context("sqlite reorder_playlist_item")
            }
            Self::Postgres { pg_pool, .. } => {
                let mut tx = pg_pool.begin().await.context("postgres reorder_playlist_item begin")?;
                let row = sqlx::query(
                    "SELECT playlist_id, position
                     FROM playlist_items
                     WHERE id = $1",
                )
                .bind(item_id)
                .fetch_one(&mut *tx)
                .await
                .context("postgres reorder_playlist_item select")?;
                let playlist_id: i64 = row.get("playlist_id");
                let current_position: i32 = row.get("position");
                let new_position = current_position + direction;
                if new_position < 0 {
                    tx.commit()
                        .await
                        .context("postgres reorder_playlist_item noop commit")?;
                    return Ok(());
                }
                sqlx::query(
                    "UPDATE playlist_items
                     SET position = $1
                     WHERE playlist_id = $2 AND position = $3",
                )
                .bind(current_position)
                .bind(playlist_id)
                .bind(new_position)
                .execute(&mut *tx)
                .await
                .context("postgres reorder_playlist_item swap")?;
                sqlx::query("UPDATE playlist_items SET position = $1 WHERE id = $2")
                    .bind(new_position)
                    .bind(item_id)
                    .execute(&mut *tx)
                    .await
                    .context("postgres reorder_playlist_item set")?;
                sqlx::query("UPDATE playlists SET updated_at = NOW() WHERE id = $1")
                    .bind(playlist_id)
                    .execute(&mut *tx)
                    .await
                    .context("postgres reorder_playlist_item touch_playlist")?;
                tx.commit().await.context("postgres reorder_playlist_item commit")?;
                Ok(())
            }
        }
    }

    pub async fn get_playlist_items(&self, playlist_id: i64) -> Result<Vec<PlaylistItem>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_playlist_items connection")?;
                db::get_playlist_items(&conn, playlist_id).context("sqlite get_playlist_items")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, playlist_id, position, download_history_id, title, artist, url,
                            duration_secs, file_id, source, CAST(added_at AS TEXT) AS added_at
                     FROM playlist_items
                     WHERE playlist_id = $1
                     ORDER BY position",
                )
                .bind(playlist_id)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_playlist_items")?;
                rows.into_iter().map(map_pg_playlist_item).collect()
            }
        }
    }

    pub async fn get_playlist_items_page(
        &self,
        playlist_id: i64,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<PlaylistItem>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_playlist_items_page connection")?;
                db::get_playlist_items_page(&conn, playlist_id, offset, limit).context("sqlite get_playlist_items_page")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, playlist_id, position, download_history_id, title, artist, url,
                            duration_secs, file_id, source, CAST(added_at AS TEXT) AS added_at
                     FROM playlist_items
                     WHERE playlist_id = $1
                     ORDER BY position
                     LIMIT $2 OFFSET $3",
                )
                .bind(playlist_id)
                .bind(limit)
                .bind(offset)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_playlist_items_page")?;
                rows.into_iter().map(map_pg_playlist_item).collect()
            }
        }
    }

    pub async fn count_playlist_items(&self, playlist_id: i64) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite count_playlist_items connection")?;
                db::count_playlist_items(&conn, playlist_id).context("sqlite count_playlist_items")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT COUNT(*)::BIGINT AS count
                     FROM playlist_items
                     WHERE playlist_id = $1",
                )
                .bind(playlist_id)
                .fetch_one(pg_pool)
                .await
                .context("postgres count_playlist_items")?;
                Ok(row.get("count"))
            }
        }
    }

    pub async fn update_playlist_item_file_id(&self, item_id: i64, file_id: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite update_playlist_item_file_id connection")?;
                db::update_item_file_id(&conn, item_id, file_id).context("sqlite update_playlist_item_file_id")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("UPDATE playlist_items SET file_id = $2 WHERE id = $1")
                    .bind(item_id)
                    .bind(file_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres update_playlist_item_file_id")?;
                Ok(())
            }
        }
    }

    pub async fn create_synced_playlist(
        &self,
        user_id: i64,
        name: &str,
        description: Option<&str>,
        source_url: &str,
        source_platform: &str,
        track_count: i32,
        matched_count: i32,
        not_found_count: i32,
    ) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite create_synced_playlist connection")?;
                db::create_synced_playlist(
                    &conn,
                    user_id,
                    name,
                    description,
                    source_url,
                    source_platform,
                    track_count,
                    matched_count,
                    not_found_count,
                )
                .context("sqlite create_synced_playlist")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "INSERT INTO synced_playlists (
                        user_id, name, description, source_url, source_platform,
                        track_count, matched_count, not_found_count, created_at, updated_at
                     ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW(), NOW())
                     RETURNING id",
                )
                .bind(user_id)
                .bind(name)
                .bind(description)
                .bind(source_url)
                .bind(source_platform)
                .bind(track_count)
                .bind(matched_count)
                .bind(not_found_count)
                .fetch_one(pg_pool)
                .await
                .context("postgres create_synced_playlist")?;
                Ok(row.get("id"))
            }
        }
    }

    pub async fn get_synced_playlist(&self, playlist_id: i64) -> Result<Option<SyncedPlaylist>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_synced_playlist connection")?;
                db::get_synced_playlist(&conn, playlist_id).context("sqlite get_synced_playlist")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT id, user_id, name, description, source_url, source_platform,
                            track_count, matched_count, not_found_count, sync_enabled,
                            CAST(last_synced_at AS TEXT) AS last_synced_at,
                            CAST(created_at AS TEXT) AS created_at,
                            CAST(updated_at AS TEXT) AS updated_at
                     FROM synced_playlists
                     WHERE id = $1",
                )
                .bind(playlist_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_synced_playlist")?;
                row.map(map_pg_synced_playlist).transpose()
            }
        }
    }

    pub async fn get_user_synced_playlists(&self, user_id: i64) -> Result<Vec<SyncedPlaylist>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user_synced_playlists connection")?;
                db::get_user_synced_playlists(&conn, user_id).context("sqlite get_user_synced_playlists")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, user_id, name, description, source_url, source_platform,
                            track_count, matched_count, not_found_count, sync_enabled,
                            CAST(last_synced_at AS TEXT) AS last_synced_at,
                            CAST(created_at AS TEXT) AS created_at,
                            CAST(updated_at AS TEXT) AS updated_at
                     FROM synced_playlists
                     WHERE user_id = $1
                     ORDER BY created_at DESC",
                )
                .bind(user_id)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_user_synced_playlists")?;
                rows.into_iter().map(map_pg_synced_playlist).collect()
            }
        }
    }

    pub async fn get_synced_playlist_by_url(&self, user_id: i64, source_url: &str) -> Result<Option<SyncedPlaylist>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_synced_playlist_by_url connection")?;
                db::get_synced_playlist_by_url(&conn, user_id, source_url).context("sqlite get_synced_playlist_by_url")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT id, user_id, name, description, source_url, source_platform,
                            track_count, matched_count, not_found_count, sync_enabled,
                            CAST(last_synced_at AS TEXT) AS last_synced_at,
                            CAST(created_at AS TEXT) AS created_at,
                            CAST(updated_at AS TEXT) AS updated_at
                     FROM synced_playlists
                     WHERE user_id = $1 AND source_url = $2",
                )
                .bind(user_id)
                .bind(source_url)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_synced_playlist_by_url")?;
                row.map(map_pg_synced_playlist).transpose()
            }
        }
    }

    pub async fn count_user_synced_playlists(&self, user_id: i64) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite count_user_synced_playlists connection")?;
                db::count_user_synced_playlists(&conn, user_id).context("sqlite count_user_synced_playlists")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT COUNT(*)::BIGINT AS count
                     FROM synced_playlists
                     WHERE user_id = $1",
                )
                .bind(user_id)
                .fetch_one(pg_pool)
                .await
                .context("postgres count_user_synced_playlists")?;
                Ok(row.get("count"))
            }
        }
    }

    pub async fn update_synced_playlist_counts(
        &self,
        playlist_id: i64,
        track_count: i32,
        matched_count: i32,
        not_found_count: i32,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite update_synced_playlist_counts connection")?;
                db::update_synced_playlist_counts(&conn, playlist_id, track_count, matched_count, not_found_count)
                    .context("sqlite update_synced_playlist_counts")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE synced_playlists
                     SET track_count = $2,
                         matched_count = $3,
                         not_found_count = $4,
                         last_synced_at = NOW(),
                         updated_at = NOW()
                     WHERE id = $1",
                )
                .bind(playlist_id)
                .bind(track_count)
                .bind(matched_count)
                .bind(not_found_count)
                .execute(pg_pool)
                .await
                .context("postgres update_synced_playlist_counts")?;
                Ok(())
            }
        }
    }

    pub async fn delete_synced_playlist(&self, playlist_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite delete_synced_playlist connection")?;
                db::delete_synced_playlist(&conn, playlist_id).context("sqlite delete_synced_playlist")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("DELETE FROM synced_playlists WHERE id = $1")
                    .bind(playlist_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres delete_synced_playlist")?;
                Ok(())
            }
        }
    }

    pub async fn increment_synced_playlist_matched(&self, playlist_id: i64, delta: i32) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn =
                    db::get_connection(db_pool).context("sqlite increment_synced_playlist_matched connection")?;
                db::increment_synced_playlist_matched(&conn, playlist_id, delta)
                    .context("sqlite increment_synced_playlist_matched")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE synced_playlists
                     SET matched_count = matched_count + $2,
                         not_found_count = not_found_count - $2,
                         updated_at = NOW()
                     WHERE id = $1",
                )
                .bind(playlist_id)
                .bind(delta)
                .execute(pg_pool)
                .await
                .context("postgres increment_synced_playlist_matched")?;
                Ok(())
            }
        }
    }

    pub async fn add_synced_track(
        &self,
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
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite add_synced_track connection")?;
                db::add_synced_track(
                    &conn,
                    playlist_id,
                    position,
                    title,
                    artist,
                    duration_secs,
                    external_id,
                    source_url,
                    resolved_url,
                    import_status,
                )
                .context("sqlite add_synced_track")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "INSERT INTO synced_tracks (
                        playlist_id, position, title, artist, duration_secs, external_id,
                        source_url, resolved_url, import_status, added_at
                     ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
                     RETURNING id",
                )
                .bind(playlist_id)
                .bind(position)
                .bind(title)
                .bind(artist)
                .bind(duration_secs)
                .bind(external_id)
                .bind(source_url)
                .bind(resolved_url)
                .bind(import_status)
                .fetch_one(pg_pool)
                .await
                .context("postgres add_synced_track")?;
                Ok(row.get("id"))
            }
        }
    }

    pub async fn get_synced_tracks(&self, playlist_id: i64) -> Result<Vec<SyncedTrack>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_synced_tracks connection")?;
                db::get_synced_tracks(&conn, playlist_id).context("sqlite get_synced_tracks")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, playlist_id, position, title, artist, duration_secs, external_id,
                            source_url, resolved_url, import_status, file_id,
                            CAST(added_at AS TEXT) AS added_at
                     FROM synced_tracks
                     WHERE playlist_id = $1
                     ORDER BY position ASC",
                )
                .bind(playlist_id)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_synced_tracks")?;
                rows.into_iter().map(map_pg_synced_track).collect()
            }
        }
    }

    pub async fn get_synced_tracks_page(&self, playlist_id: i64, offset: i64, limit: i64) -> Result<Vec<SyncedTrack>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_synced_tracks_page connection")?;
                db::get_synced_tracks_page(&conn, playlist_id, offset, limit).context("sqlite get_synced_tracks_page")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, playlist_id, position, title, artist, duration_secs, external_id,
                            source_url, resolved_url, import_status, file_id,
                            CAST(added_at AS TEXT) AS added_at
                     FROM synced_tracks
                     WHERE playlist_id = $1
                     ORDER BY position ASC
                     LIMIT $2 OFFSET $3",
                )
                .bind(playlist_id)
                .bind(limit)
                .bind(offset)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_synced_tracks_page")?;
                rows.into_iter().map(map_pg_synced_track).collect()
            }
        }
    }

    pub async fn get_synced_track(&self, track_id: i64) -> Result<Option<SyncedTrack>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_synced_track connection")?;
                db::get_synced_track(&conn, track_id).context("sqlite get_synced_track")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT id, playlist_id, position, title, artist, duration_secs, external_id,
                            source_url, resolved_url, import_status, file_id,
                            CAST(added_at AS TEXT) AS added_at
                     FROM synced_tracks
                     WHERE id = $1",
                )
                .bind(track_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_synced_track")?;
                row.map(map_pg_synced_track).transpose()
            }
        }
    }

    pub async fn update_synced_track_file_id(&self, track_id: i64, file_id: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite update_synced_track_file_id connection")?;
                db::update_synced_track_file_id(&conn, track_id, file_id).context("sqlite update_synced_track_file_id")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("UPDATE synced_tracks SET file_id = $2 WHERE id = $1")
                    .bind(track_id)
                    .bind(file_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres update_synced_track_file_id")?;
                Ok(())
            }
        }
    }

    pub async fn update_synced_track_status(
        &self,
        track_id: i64,
        status: &str,
        resolved_url: Option<&str>,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite update_synced_track_status connection")?;
                db::update_synced_track_status(&conn, track_id, status, resolved_url)
                    .context("sqlite update_synced_track_status")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE synced_tracks
                     SET import_status = $2, resolved_url = $3
                     WHERE id = $1",
                )
                .bind(track_id)
                .bind(status)
                .bind(resolved_url)
                .execute(pg_pool)
                .await
                .context("postgres update_synced_track_status")?;
                Ok(())
            }
        }
    }

    pub async fn delete_synced_tracks(&self, playlist_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite delete_synced_tracks connection")?;
                db::delete_synced_tracks(&conn, playlist_id).context("sqlite delete_synced_tracks")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("DELETE FROM synced_tracks WHERE playlist_id = $1")
                    .bind(playlist_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres delete_synced_tracks")?;
                Ok(())
            }
        }
    }

    pub async fn count_synced_tracks(&self, playlist_id: i64) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite count_synced_tracks connection")?;
                db::count_synced_tracks(&conn, playlist_id).context("sqlite count_synced_tracks")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT COUNT(*)::BIGINT AS count
                     FROM synced_tracks
                     WHERE playlist_id = $1",
                )
                .bind(playlist_id)
                .fetch_one(pg_pool)
                .await
                .context("postgres count_synced_tracks")?;
                Ok(row.get("count"))
            }
        }
    }

    pub async fn create_audio_effect_session(&self, session: &AudioEffectSession) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite create_audio_effect_session connection")?;
                db::create_audio_effect_session(&conn, session).context("sqlite create_audio_effect_session")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO audio_effect_sessions (
                        id, user_id, original_file_path, current_file_path, telegram_file_id,
                        original_message_id, title, duration, pitch_semitones, tempo_factor,
                        bass_gain_db, morph_profile, version, processing, created_at, expires_at
                     ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
                )
                .bind(&session.id)
                .bind(session.user_id)
                .bind(&session.original_file_path)
                .bind(&session.current_file_path)
                .bind(&session.telegram_file_id)
                .bind(session.original_message_id)
                .bind(&session.title)
                .bind(session.duration as i64)
                .bind(session.pitch_semitones as i16)
                .bind(session.tempo_factor as f64)
                .bind(session.bass_gain_db as i16)
                .bind(session.morph_profile.as_str())
                .bind(session.version as i64)
                .bind(if session.processing { 1_i32 } else { 0_i32 })
                .bind(session.created_at)
                .bind(session.expires_at)
                .execute(pg_pool)
                .await
                .context("postgres create_audio_effect_session")?;
                Ok(())
            }
        }
    }

    pub async fn get_audio_effect_session(&self, session_id: &str) -> Result<Option<AudioEffectSession>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_audio_effect_session connection")?;
                db::get_audio_effect_session(&conn, session_id).context("sqlite get_audio_effect_session")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT *
                     FROM audio_effect_sessions
                     WHERE id = $1",
                )
                .bind(session_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_audio_effect_session")?;
                row.map(map_pg_audio_effect_session).transpose()
            }
        }
    }

    pub async fn delete_expired_audio_sessions(&self) -> Result<Vec<AudioEffectSession>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite delete_expired_audio_sessions connection")?;
                db::delete_expired_audio_sessions(&conn).context("sqlite delete_expired_audio_sessions")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "DELETE FROM audio_effect_sessions
                     WHERE expires_at < NOW()
                     RETURNING *",
                )
                .fetch_all(pg_pool)
                .await
                .context("postgres delete_expired_audio_sessions")?;
                rows.into_iter().map(map_pg_audio_effect_session).collect()
            }
        }
    }

    pub async fn update_audio_effect_session(
        &self,
        session_id: &str,
        pitch_semitones: i8,
        tempo_factor: f32,
        bass_gain_db: i8,
        morph_profile: &str,
        current_file_path: &str,
        version: u32,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite update_audio_effect_session connection")?;
                db::update_audio_effect_session(
                    &conn,
                    session_id,
                    pitch_semitones,
                    tempo_factor,
                    bass_gain_db,
                    morph_profile,
                    current_file_path,
                    version,
                )
                .context("sqlite update_audio_effect_session")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE audio_effect_sessions
                     SET pitch_semitones = $1,
                         tempo_factor = $2,
                         bass_gain_db = $3,
                         morph_profile = $4,
                         current_file_path = $5,
                         version = $6
                     WHERE id = $7",
                )
                .bind(pitch_semitones as i16)
                .bind(tempo_factor as f64)
                .bind(bass_gain_db as i16)
                .bind(morph_profile)
                .bind(current_file_path)
                .bind(version as i64)
                .bind(session_id)
                .execute(pg_pool)
                .await
                .context("postgres update_audio_effect_session")?;
                Ok(())
            }
        }
    }

    pub async fn set_audio_effect_session_processing(&self, session_id: &str, processing: bool) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite set_session_processing connection")?;
                db::set_session_processing(&conn, session_id, processing).context("sqlite set_session_processing")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE audio_effect_sessions
                     SET processing = $1
                     WHERE id = $2",
                )
                .bind(if processing { 1_i32 } else { 0_i32 })
                .bind(session_id)
                .execute(pg_pool)
                .await
                .context("postgres set_session_processing")?;
                Ok(())
            }
        }
    }

    pub async fn upsert_audio_cut_session(&self, session: &AudioCutSession) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite upsert_audio_cut_session connection")?;
                db::upsert_audio_cut_session(&conn, session).context("sqlite upsert_audio_cut_session")
            }
            Self::Postgres { pg_pool, .. } => {
                let mut tx = pg_pool
                    .begin()
                    .await
                    .context("postgres upsert_audio_cut_session begin")?;
                sqlx::query("DELETE FROM audio_cut_sessions WHERE user_id = $1")
                    .bind(session.user_id)
                    .execute(&mut *tx)
                    .await
                    .context("postgres upsert_audio_cut_session delete")?;
                sqlx::query(
                    "INSERT INTO audio_cut_sessions (id, user_id, audio_session_id, created_at, expires_at)
                     VALUES ($1, $2, $3, $4, $5)",
                )
                .bind(&session.id)
                .bind(session.user_id)
                .bind(&session.audio_session_id)
                .bind(session.created_at)
                .bind(session.expires_at)
                .execute(&mut *tx)
                .await
                .context("postgres upsert_audio_cut_session insert")?;
                tx.commit().await.context("postgres upsert_audio_cut_session commit")?;
                Ok(())
            }
        }
    }

    pub async fn get_active_audio_cut_session(&self, user_id: i64) -> Result<Option<AudioCutSession>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_active_audio_cut_session connection")?;
                db::get_active_audio_cut_session(&conn, user_id).context("sqlite get_active_audio_cut_session")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT *
                     FROM audio_cut_sessions
                     WHERE user_id = $1
                       AND expires_at > NOW()
                     ORDER BY created_at DESC
                     LIMIT 1",
                )
                .bind(user_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_active_audio_cut_session")?;
                row.map(map_pg_audio_cut_session).transpose()
            }
        }
    }

    pub async fn delete_audio_cut_session_by_user(&self, user_id: i64) -> Result<()> {
        self.delete_session_by_user(
            user_id,
            "audio_cut_sessions",
            "sqlite delete_audio_cut_session_by_user connection",
            db::delete_audio_cut_session_by_user,
        )
        .await
    }

    pub async fn upsert_video_clip_session(&self, session: &VideoClipSession) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite upsert_video_clip_session connection")?;
                db::upsert_video_clip_session(&conn, session).context("sqlite upsert_video_clip_session")
            }
            Self::Postgres { pg_pool, .. } => {
                let mut tx = pg_pool
                    .begin()
                    .await
                    .context("postgres upsert_video_clip_session begin")?;
                sqlx::query("DELETE FROM video_clip_sessions WHERE user_id = $1")
                    .bind(session.user_id)
                    .execute(&mut *tx)
                    .await
                    .context("postgres upsert_video_clip_session delete")?;
                sqlx::query(
                    "INSERT INTO video_clip_sessions (
                        id, user_id, source_download_id, source_kind, source_id, original_url,
                        output_kind, created_at, expires_at, subtitle_lang
                     ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
                )
                .bind(&session.id)
                .bind(session.user_id)
                .bind(session.source_download_id)
                .bind(&session.source_kind)
                .bind(session.source_id)
                .bind(&session.original_url)
                .bind(&session.output_kind)
                .bind(session.created_at)
                .bind(session.expires_at)
                .bind(&session.subtitle_lang)
                .execute(&mut *tx)
                .await
                .context("postgres upsert_video_clip_session insert")?;
                tx.commit().await.context("postgres upsert_video_clip_session commit")?;
                Ok(())
            }
        }
    }

    pub async fn get_active_video_clip_session(&self, user_id: i64) -> Result<Option<VideoClipSession>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_active_video_clip_session connection")?;
                db::get_active_video_clip_session(&conn, user_id).context("sqlite get_active_video_clip_session")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT *
                     FROM video_clip_sessions
                     WHERE user_id = $1
                       AND expires_at > NOW()
                     ORDER BY created_at DESC
                     LIMIT 1",
                )
                .bind(user_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_active_video_clip_session")?;
                row.map(map_pg_video_clip_session).transpose()
            }
        }
    }

    pub async fn delete_video_clip_session_by_user(&self, user_id: i64) -> Result<()> {
        self.delete_session_by_user(
            user_id,
            "video_clip_sessions",
            "sqlite delete_video_clip_session_by_user connection",
            db::delete_video_clip_session_by_user,
        )
        .await
    }

    pub async fn upsert_cookies_upload_session(&self, session: &CookiesUploadSession) -> Result<()> {
        self.upsert_cookies_session(session, false).await
    }

    pub async fn get_active_cookies_upload_session(&self, user_id: i64) -> Result<Option<CookiesUploadSession>> {
        self.get_active_cookies_session(user_id, false).await
    }

    pub async fn delete_cookies_upload_session_by_user(&self, user_id: i64) -> Result<()> {
        self.delete_session_by_user(
            user_id,
            "cookies_upload_sessions",
            "sqlite delete_cookies_upload_session_by_user connection",
            db::delete_cookies_upload_session_by_user,
        )
        .await
    }

    pub async fn upsert_ig_cookies_upload_session(&self, session: &CookiesUploadSession) -> Result<()> {
        self.upsert_cookies_session(session, true).await
    }

    pub async fn get_active_ig_cookies_upload_session(&self, user_id: i64) -> Result<Option<CookiesUploadSession>> {
        self.get_active_cookies_session(user_id, true).await
    }

    pub async fn delete_ig_cookies_upload_session_by_user(&self, user_id: i64) -> Result<()> {
        self.delete_session_by_user(
            user_id,
            "ig_cookies_upload_sessions",
            "sqlite delete_ig_cookies_upload_session_by_user connection",
            db::delete_ig_cookies_upload_session_by_user,
        )
        .await
    }

    pub async fn create_lyrics_session(
        &self,
        id: &str,
        user_id: i64,
        artist: &str,
        title: &str,
        sections_json: &str,
        has_structure: bool,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite create_lyrics_session connection")?;
                db::create_lyrics_session(&conn, id, user_id, artist, title, sections_json, has_structure)
                    .context("sqlite create_lyrics_session")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO lyrics_sessions (
                        id, user_id, artist, title, sections_json, has_structure, created_at, expires_at
                     ) VALUES ($1, $2, $3, $4, $5, $6, NOW(), NOW() + INTERVAL '24 hours')",
                )
                .bind(id)
                .bind(user_id)
                .bind(artist)
                .bind(title)
                .bind(sections_json)
                .bind(if has_structure { 1_i32 } else { 0_i32 })
                .execute(pg_pool)
                .await
                .context("postgres create_lyrics_session")?;
                Ok(())
            }
        }
    }

    pub async fn get_lyrics_session(&self, id: &str) -> Result<Option<(String, String, String, bool)>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_lyrics_session connection")?;
                db::get_lyrics_session(&conn, id).context("sqlite get_lyrics_session")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT artist, title, sections_json, has_structure
                     FROM lyrics_sessions
                     WHERE id = $1
                       AND expires_at > NOW()",
                )
                .bind(id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_lyrics_session")?;
                Ok(row.map(|row| {
                    (
                        row.get("artist"),
                        row.get("title"),
                        row.get("sections_json"),
                        row.get::<i32, _>("has_structure") != 0,
                    )
                }))
            }
        }
    }

    pub async fn create_player_session(
        &self,
        user_id: i64,
        playlist_id: i64,
        player_message_id: Option<i32>,
        sticker_message_id: Option<i32>,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite create_player_session connection")?;
                db::create_player_session(&conn, user_id, playlist_id, player_message_id, sticker_message_id)
                    .context("sqlite create_player_session")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO player_sessions (
                        user_id, playlist_id, current_position, is_shuffle, player_message_id, sticker_message_id, updated_at
                     ) VALUES ($1, $2, 0, 0, $3, $4, NOW())
                     ON CONFLICT (user_id) DO UPDATE SET
                        playlist_id = EXCLUDED.playlist_id,
                        current_position = 0,
                        is_shuffle = 0,
                        player_message_id = EXCLUDED.player_message_id,
                        sticker_message_id = EXCLUDED.sticker_message_id,
                        updated_at = NOW()",
                )
                .bind(user_id)
                .bind(playlist_id)
                .bind(player_message_id)
                .bind(sticker_message_id)
                .execute(pg_pool)
                .await
                .context("postgres create_player_session")?;
                Ok(())
            }
        }
    }

    pub async fn get_player_session(&self, user_id: i64) -> Result<Option<PlayerSession>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_player_session connection")?;
                db::get_player_session(&conn, user_id).context("sqlite get_player_session")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT user_id, playlist_id, current_position, is_shuffle, player_message_id, sticker_message_id,
                            updated_at::text AS updated_at
                     FROM player_sessions
                     WHERE user_id = $1",
                )
                .bind(user_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_player_session")?;
                Ok(row.map(|row| PlayerSession {
                    user_id: row.get("user_id"),
                    playlist_id: row.get("playlist_id"),
                    current_position: row.get("current_position"),
                    is_shuffle: row.get::<i32, _>("is_shuffle") != 0,
                    player_message_id: row.get("player_message_id"),
                    sticker_message_id: row.get("sticker_message_id"),
                    updated_at: row.get("updated_at"),
                }))
            }
        }
    }

    pub async fn toggle_player_shuffle(&self, user_id: i64) -> Result<bool> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite toggle_player_shuffle connection")?;
                db::toggle_player_shuffle(&conn, user_id).context("sqlite toggle_player_shuffle")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "UPDATE player_sessions
                     SET is_shuffle = CASE WHEN is_shuffle = 0 THEN 1 ELSE 0 END,
                         updated_at = NOW()
                     WHERE user_id = $1
                     RETURNING is_shuffle",
                )
                .bind(user_id)
                .fetch_one(pg_pool)
                .await
                .context("postgres toggle_player_shuffle")?;
                Ok(row.get::<i32, _>("is_shuffle") != 0)
            }
        }
    }

    pub async fn delete_player_session(&self, user_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite delete_player_session connection")?;
                db::delete_player_session(&conn, user_id).context("sqlite delete_player_session")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("DELETE FROM player_sessions WHERE user_id = $1")
                    .bind(user_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres delete_player_session")?;
                Ok(())
            }
        }
    }

    pub async fn add_player_message(&self, user_id: i64, message_id: i32) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite add_player_message connection")?;
                db::add_player_message(&conn, user_id, message_id).context("sqlite add_player_message")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO player_messages (user_id, message_id)
                     VALUES ($1, $2)
                     ON CONFLICT (user_id, message_id) DO NOTHING",
                )
                .bind(user_id)
                .bind(message_id)
                .execute(pg_pool)
                .await
                .context("postgres add_player_message")?;
                Ok(())
            }
        }
    }

    pub async fn get_player_messages(&self, user_id: i64) -> Result<Vec<i32>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_player_messages connection")?;
                db::get_player_messages(&conn, user_id).context("sqlite get_player_messages")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query("SELECT message_id FROM player_messages WHERE user_id = $1")
                    .bind(user_id)
                    .fetch_all(pg_pool)
                    .await
                    .context("postgres get_player_messages")?;
                Ok(rows.into_iter().map(|row| row.get("message_id")).collect())
            }
        }
    }

    pub async fn delete_player_messages(&self, user_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite delete_player_messages connection")?;
                db::delete_player_messages(&conn, user_id).context("sqlite delete_player_messages")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("DELETE FROM player_messages WHERE user_id = $1")
                    .bind(user_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres delete_player_messages")?;
                Ok(())
            }
        }
    }

    pub async fn create_new_category_session(&self, user_id: i64, download_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite create_new_category_session connection")?;
                db::create_new_category_session(&conn, user_id, download_id)
                    .context("sqlite create_new_category_session")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO new_category_sessions (user_id, download_id, created_at)
                     VALUES ($1, $2, NOW())
                     ON CONFLICT (user_id) DO UPDATE SET
                        download_id = EXCLUDED.download_id,
                        created_at = NOW()",
                )
                .bind(user_id)
                .bind(download_id)
                .execute(pg_pool)
                .await
                .context("postgres create_new_category_session")?;
                Ok(())
            }
        }
    }

    pub async fn get_active_new_category_session(&self, user_id: i64) -> Result<Option<i64>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_active_new_category_session connection")?;
                db::get_active_new_category_session(&conn, user_id).context("sqlite get_active_new_category_session")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT download_id
                     FROM new_category_sessions
                     WHERE user_id = $1
                       AND created_at > NOW() - INTERVAL '10 minutes'",
                )
                .bind(user_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_active_new_category_session")?;
                Ok(row.map(|row| row.get("download_id")))
            }
        }
    }

    pub async fn delete_new_category_session(&self, user_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite delete_new_category_session connection")?;
                db::delete_new_category_session(&conn, user_id).context("sqlite delete_new_category_session")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("DELETE FROM new_category_sessions WHERE user_id = $1")
                    .bind(user_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres delete_new_category_session")?;
                Ok(())
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn save_download_history(
        &self,
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
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite save_download_history connection")?;
                db::save_download_history(
                    &conn,
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
                    source_id,
                    part_index,
                )
                .context("sqlite save_download_history")
            }
            Self::Postgres { pg_pool, .. } => {
                let bot_api_url = config::bot_api::get_url();
                let bot_api_is_local = if config::bot_api::is_local() { 1 } else { 0 };
                let row = sqlx::query(
                    "INSERT INTO download_history (
                        user_id, url, title, format, file_id, author, file_size, duration,
                        video_quality, audio_bitrate, bot_api_url, bot_api_is_local, source_id, part_index
                     ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
                     RETURNING id",
                )
                .bind(telegram_id)
                .bind(url)
                .bind(title)
                .bind(format)
                .bind(file_id)
                .bind(author)
                .bind(file_size)
                .bind(duration)
                .bind(video_quality)
                .bind(audio_bitrate)
                .bind(bot_api_url)
                .bind(bot_api_is_local)
                .bind(source_id)
                .bind(part_index)
                .fetch_one(pg_pool)
                .await
                .context("postgres save_download_history")?;
                Ok(row.get("id"))
            }
        }
    }

    pub async fn update_download_message_id(&self, download_id: i64, message_id: i32, chat_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite update_download_message_id connection")?;
                db::update_download_message_id(&conn, download_id, message_id, chat_id)
                    .context("sqlite update_download_message_id")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("UPDATE download_history SET message_id = $2, chat_id = $3 WHERE id = $1")
                    .bind(download_id)
                    .bind(message_id)
                    .bind(chat_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres update_download_message_id")?;
                Ok(())
            }
        }
    }

    pub async fn get_download_message_info(&self, download_id: i64) -> Result<Option<(i32, i64)>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_download_message_info connection")?;
                db::get_download_message_info(&conn, download_id).context("sqlite get_download_message_info")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT message_id, chat_id FROM download_history
                     WHERE id = $1 AND message_id IS NOT NULL AND chat_id IS NOT NULL",
                )
                .bind(download_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_download_message_info")?;
                Ok(row.map(|row| (row.get("message_id"), row.get("chat_id"))))
            }
        }
    }

    pub async fn get_download_history(
        &self,
        telegram_id: i64,
        limit: Option<i32>,
    ) -> Result<Vec<DownloadHistoryEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_download_history connection")?;
                db::get_download_history(&conn, telegram_id, limit).context("sqlite get_download_history")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, url, title, format, downloaded_at::text AS downloaded_at, file_id, author,
                            file_size, duration, video_quality, audio_bitrate, bot_api_url, bot_api_is_local,
                            source_id, part_index, category
                     FROM download_history
                     WHERE user_id = $1
                     ORDER BY downloaded_at DESC
                     LIMIT $2",
                )
                .bind(telegram_id)
                .bind(i64::from(limit.unwrap_or(20)))
                .fetch_all(pg_pool)
                .await
                .context("postgres get_download_history")?;
                Ok(rows.into_iter().map(map_pg_download_history).collect())
            }
        }
    }

    pub async fn find_cached_file_id(
        &self,
        url: &str,
        format: &str,
        video_quality: Option<&str>,
        audio_bitrate: Option<&str>,
    ) -> Result<Option<String>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite find_cached_file_id connection")?;
                db::find_cached_file_id(&conn, url, format, video_quality, audio_bitrate)
                    .context("sqlite find_cached_file_id")
            }
            Self::Postgres { pg_pool, .. } => {
                let current_api_url = std::env::var("BOT_API_URL").ok();
                let current_is_local = current_api_url
                    .as_deref()
                    .map(|u| !u.contains("api.telegram.org"))
                    .unwrap_or(false);
                let row = sqlx::query(
                    "SELECT file_id
                     FROM download_history
                     WHERE url = $1
                       AND format = $2
                       AND file_id IS NOT NULL
                       AND bot_api_is_local = $3
                       AND ($4::text IS NULL OR video_quality = $4)
                       AND ($5::text IS NULL OR audio_bitrate = $5)
                       AND ($6::text IS NULL OR bot_api_url = $6)
                     ORDER BY downloaded_at DESC
                     LIMIT 1",
                )
                .bind(url)
                .bind(format)
                .bind(i32::from(current_is_local))
                .bind(video_quality)
                .bind(audio_bitrate)
                .bind(current_api_url)
                .fetch_optional(pg_pool)
                .await
                .context("postgres find_cached_file_id")?;
                Ok(row.map(|row| row.get("file_id")))
            }
        }
    }

    pub async fn get_download_history_entry(
        &self,
        telegram_id: i64,
        entry_id: i64,
    ) -> Result<Option<DownloadHistoryEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_download_history_entry connection")?;
                db::get_download_history_entry(&conn, telegram_id, entry_id)
                    .context("sqlite get_download_history_entry")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT id, url, title, format, downloaded_at::text AS downloaded_at, file_id, author,
                            file_size, duration, video_quality, audio_bitrate, bot_api_url, bot_api_is_local,
                            source_id, part_index, category
                     FROM download_history
                     WHERE id = $1 AND user_id = $2",
                )
                .bind(entry_id)
                .bind(telegram_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_download_history_entry")?;
                Ok(row.map(map_pg_download_history))
            }
        }
    }

    pub async fn get_download_history_filtered(
        &self,
        user_id: i64,
        file_type_filter: Option<&str>,
        search_text: Option<&str>,
        category_filter: Option<&str>,
    ) -> Result<Vec<DownloadHistoryEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_download_history_filtered connection")?;
                db::get_download_history_filtered(&conn, user_id, file_type_filter, search_text, category_filter)
                    .context("sqlite get_download_history_filtered")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, url, title, format, downloaded_at::text AS downloaded_at, file_id, author,
                            file_size, duration, video_quality, audio_bitrate, bot_api_url, bot_api_is_local,
                            source_id, part_index, category
                     FROM download_history
                     WHERE user_id = $1
                       AND file_id IS NOT NULL
                       AND (format = 'mp3' OR format = 'mp4')
                       AND ($2::text IS NULL OR format = $2)
                       AND ($3::text IS NULL OR (title ILIKE $3 OR author ILIKE $3))
                       AND ($4::text IS NULL OR category = $4)
                     ORDER BY downloaded_at DESC",
                )
                .bind(user_id)
                .bind(file_type_filter)
                .bind(search_text.map(|s| format!("%{}%", s)))
                .bind(category_filter)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_download_history_filtered")?;
                Ok(rows.into_iter().map(map_pg_download_history).collect())
            }
        }
    }

    pub async fn delete_download_history_entry(&self, telegram_id: i64, entry_id: i64) -> Result<bool> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite delete_download_history_entry connection")?;
                db::delete_download_history_entry(&conn, telegram_id, entry_id)
                    .context("sqlite delete_download_history_entry")
            }
            Self::Postgres { pg_pool, .. } => {
                let result = sqlx::query("DELETE FROM download_history WHERE id = $1 AND user_id = $2")
                    .bind(entry_id)
                    .bind(telegram_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres delete_download_history_entry")?;
                Ok(result.rows_affected() > 0)
            }
        }
    }

    pub async fn create_user_category(&self, user_id: i64, name: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite create_user_category connection")?;
                db::create_user_category(&conn, user_id, name).context("sqlite create_user_category")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO user_categories (user_id, name)
                     VALUES ($1, $2)
                     ON CONFLICT (user_id, name) DO NOTHING",
                )
                .bind(user_id)
                .bind(name)
                .execute(pg_pool)
                .await
                .context("postgres create_user_category")?;
                Ok(())
            }
        }
    }

    pub async fn get_user_categories(&self, user_id: i64) -> Result<Vec<String>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user_categories connection")?;
                db::get_user_categories(&conn, user_id).context("sqlite get_user_categories")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query("SELECT name FROM user_categories WHERE user_id = $1 ORDER BY name")
                    .bind(user_id)
                    .fetch_all(pg_pool)
                    .await
                    .context("postgres get_user_categories")?;
                Ok(rows.into_iter().map(|row| row.get("name")).collect())
            }
        }
    }

    pub async fn set_download_category(&self, user_id: i64, download_id: i64, category: Option<&str>) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite set_download_category connection")?;
                db::set_download_category(&conn, user_id, download_id, category).context("sqlite set_download_category")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("UPDATE download_history SET category = $3 WHERE id = $1 AND user_id = $2")
                    .bind(download_id)
                    .bind(user_id)
                    .bind(category)
                    .execute(pg_pool)
                    .await
                    .context("postgres set_download_category")?;
                Ok(())
            }
        }
    }

    pub async fn get_cuts_history_filtered(
        &self,
        user_id: i64,
        search_text: Option<&str>,
    ) -> Result<Vec<DownloadHistoryEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_cuts_history_filtered connection")?;
                db::get_cuts_history_filtered(&conn, user_id, search_text).context("sqlite get_cuts_history_filtered")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, original_url AS url, title, output_kind AS format, created_at::text AS downloaded_at,
                            file_id, NULL::text AS author, file_size, duration, video_quality,
                            NULL::text AS audio_bitrate, NULL::text AS bot_api_url, 0::bigint AS bot_api_is_local,
                            source_id, NULL::integer AS part_index, NULL::text AS category
                     FROM cuts
                     WHERE user_id = $1
                       AND ($2::text IS NULL OR title ILIKE $2)
                     ORDER BY created_at DESC",
                )
                .bind(user_id)
                .bind(search_text.map(|s| format!("%{}%", s)))
                .fetch_all(pg_pool)
                .await
                .context("postgres get_cuts_history_filtered")?;
                Ok(rows.into_iter().map(map_pg_download_history).collect())
            }
        }
    }

    pub async fn save_video_timestamps(&self, download_id: i64, timestamps: &[VideoTimestamp]) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite save_video_timestamps connection")?;
                db::save_video_timestamps(&conn, download_id, timestamps).context("sqlite save_video_timestamps")
            }
            Self::Postgres { pg_pool, .. } => {
                for ts in timestamps {
                    sqlx::query(
                        "INSERT INTO video_timestamps (download_id, source, time_seconds, end_seconds, label)
                         VALUES ($1, $2, $3, $4, $5)",
                    )
                    .bind(download_id)
                    .bind(ts.source.as_str())
                    .bind(ts.time_seconds)
                    .bind(ts.end_seconds)
                    .bind(ts.label.as_deref())
                    .execute(pg_pool)
                    .await
                    .context("postgres save_video_timestamps")?;
                }
                Ok(())
            }
        }
    }

    pub async fn get_video_timestamps(&self, download_id: i64) -> Result<Vec<VideoTimestamp>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_video_timestamps connection")?;
                db::get_video_timestamps(&conn, download_id).context("sqlite get_video_timestamps")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT source, time_seconds, end_seconds, label
                     FROM video_timestamps
                     WHERE download_id = $1
                     ORDER BY time_seconds ASC",
                )
                .bind(download_id)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_video_timestamps")?;
                Ok(rows
                    .into_iter()
                    .map(|row| VideoTimestamp {
                        source: TimestampSource::parse(&row.get::<String, _>("source")),
                        time_seconds: row.get("time_seconds"),
                        end_seconds: row.get("end_seconds"),
                        label: row.get("label"),
                    })
                    .collect())
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_cut(
        &self,
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
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite create_cut connection")?;
                db::create_cut(
                    &conn,
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
                )
                .context("sqlite create_cut")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "INSERT INTO cuts (
                        user_id, original_url, source_kind, source_id, output_kind, segments_json,
                        segments_text, title, file_id, file_size, duration, video_quality
                     ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
                     RETURNING id",
                )
                .bind(user_id)
                .bind(original_url)
                .bind(source_kind)
                .bind(source_id)
                .bind(output_kind)
                .bind(segments_json)
                .bind(segments_text)
                .bind(title)
                .bind(file_id)
                .bind(file_size)
                .bind(duration)
                .bind(video_quality)
                .fetch_one(pg_pool)
                .await
                .context("postgres create_cut")?;
                Ok(row.get("id"))
            }
        }
    }

    pub async fn update_cut_message_id(&self, cut_id: i64, message_id: i32, chat_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite update_cut_message_id connection")?;
                db::update_cut_message_id(&conn, cut_id, message_id, chat_id).context("sqlite update_cut_message_id")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("UPDATE cuts SET message_id = $2, chat_id = $3 WHERE id = $1")
                    .bind(cut_id)
                    .bind(message_id)
                    .bind(chat_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres update_cut_message_id")?;
                Ok(())
            }
        }
    }

    pub async fn get_cut_message_info(&self, cut_id: i64) -> Result<Option<(i32, i64)>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_cut_message_info connection")?;
                db::get_cut_message_info(&conn, cut_id).context("sqlite get_cut_message_info")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT message_id, chat_id FROM cuts
                     WHERE id = $1 AND message_id IS NOT NULL AND chat_id IS NOT NULL",
                )
                .bind(cut_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_cut_message_info")?;
                Ok(row.map(|row| (row.get("message_id"), row.get("chat_id"))))
            }
        }
    }

    pub async fn get_cuts_count(&self, user_id: i64) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_cuts_count connection")?;
                db::get_cuts_count(&conn, user_id).context("sqlite get_cuts_count")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query("SELECT COUNT(*) AS count FROM cuts WHERE user_id = $1")
                    .bind(user_id)
                    .fetch_one(pg_pool)
                    .await
                    .context("postgres get_cuts_count")?;
                Ok(row.get("count"))
            }
        }
    }

    pub async fn get_cuts_page(&self, user_id: i64, limit: i64, offset: i64) -> Result<Vec<CutEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_cuts_page connection")?;
                db::get_cuts_page(&conn, user_id, limit, offset).context("sqlite get_cuts_page")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "SELECT id, user_id, original_url, source_kind, source_id, output_kind, segments_json,
                            segments_text, title, created_at::text AS created_at, file_id, file_size, duration,
                            video_quality
                     FROM cuts
                     WHERE user_id = $1
                     ORDER BY created_at DESC
                     LIMIT $2 OFFSET $3",
                )
                .bind(user_id)
                .bind(limit)
                .bind(offset)
                .fetch_all(pg_pool)
                .await
                .context("postgres get_cuts_page")?;
                Ok(rows.into_iter().map(map_pg_cut).collect())
            }
        }
    }

    pub async fn get_cut_entry(&self, user_id: i64, cut_id: i64) -> Result<Option<CutEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_cut_entry connection")?;
                db::get_cut_entry(&conn, user_id, cut_id).context("sqlite get_cut_entry")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT id, user_id, original_url, source_kind, source_id, output_kind, segments_json,
                            segments_text, title, created_at::text AS created_at, file_id, file_size, duration,
                            video_quality
                     FROM cuts
                     WHERE id = $1 AND user_id = $2",
                )
                .bind(cut_id)
                .bind(user_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_cut_entry")?;
                Ok(row.map(map_pg_cut))
            }
        }
    }

    pub async fn get_user_vault(&self, user_id: i64) -> Result<Option<UserVault>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user_vault connection")?;
                db::get_user_vault(&conn, user_id).context("sqlite get_user_vault")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT user_id, channel_id, channel_title, is_active, created_at::text AS created_at
                     FROM user_vaults
                     WHERE user_id = $1",
                )
                .bind(user_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_user_vault")?;
                Ok(row.map(|row| UserVault {
                    user_id: row.get("user_id"),
                    channel_id: row.get("channel_id"),
                    channel_title: row.get("channel_title"),
                    is_active: row.get::<i32, _>("is_active") != 0,
                    created_at: row.get("created_at"),
                }))
            }
        }
    }

    pub async fn set_user_vault(&self, user_id: i64, channel_id: i64, channel_title: Option<&str>) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite set_user_vault connection")?;
                db::set_user_vault(&conn, user_id, channel_id, channel_title).context("sqlite set_user_vault")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO user_vaults (user_id, channel_id, channel_title, is_active, created_at, updated_at)
                     VALUES ($1, $2, $3, 1, NOW(), NOW())
                     ON CONFLICT (user_id) DO UPDATE SET
                        channel_id = EXCLUDED.channel_id,
                        channel_title = EXCLUDED.channel_title,
                        is_active = 1,
                        updated_at = NOW()",
                )
                .bind(user_id)
                .bind(channel_id)
                .bind(channel_title)
                .execute(pg_pool)
                .await
                .context("postgres set_user_vault")?;
                Ok(())
            }
        }
    }

    pub async fn deactivate_user_vault(&self, user_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite deactivate_user_vault connection")?;
                db::deactivate_user_vault(&conn, user_id).context("sqlite deactivate_user_vault")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("UPDATE user_vaults SET is_active = 0, updated_at = NOW() WHERE user_id = $1")
                    .bind(user_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres deactivate_user_vault")?;
                Ok(())
            }
        }
    }

    pub async fn activate_user_vault(&self, user_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite activate_user_vault connection")?;
                db::activate_user_vault(&conn, user_id).context("sqlite activate_user_vault")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("UPDATE user_vaults SET is_active = 1, updated_at = NOW() WHERE user_id = $1")
                    .bind(user_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres activate_user_vault")?;
                Ok(())
            }
        }
    }

    pub async fn delete_user_vault(&self, user_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite delete_user_vault connection")?;
                db::delete_user_vault(&conn, user_id).context("sqlite delete_user_vault")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("DELETE FROM user_vaults WHERE user_id = $1")
                    .bind(user_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres delete_user_vault")?;
                Ok(())
            }
        }
    }

    pub async fn get_vault_cached_file_id(&self, user_id: i64, url: &str) -> Result<Option<String>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_vault_cached_file_id connection")?;
                Ok(db::get_vault_cached_file_id(&conn, user_id, url))
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query("SELECT file_id FROM vault_cache WHERE user_id = $1 AND url = $2")
                    .bind(user_id)
                    .bind(url)
                    .fetch_optional(pg_pool)
                    .await
                    .context("postgres get_vault_cached_file_id")?;
                Ok(row.map(|row| row.get("file_id")))
            }
        }
    }

    pub async fn save_vault_cache_entry(
        &self,
        user_id: i64,
        url: &str,
        title: Option<&str>,
        artist: Option<&str>,
        duration_secs: Option<i32>,
        file_id: &str,
        message_id: Option<i64>,
        file_size: Option<i64>,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite save_vault_cache_entry connection")?;
                db::save_vault_cache_entry(
                    &conn,
                    user_id,
                    url,
                    title,
                    artist,
                    duration_secs,
                    file_id,
                    message_id,
                    file_size,
                )
                .context("sqlite save_vault_cache_entry")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO vault_cache (
                        user_id, url, title, artist, duration_secs, file_id, message_id, file_size, created_at
                     ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW())
                     ON CONFLICT (user_id, url) DO UPDATE SET
                        title = EXCLUDED.title,
                        artist = EXCLUDED.artist,
                        duration_secs = EXCLUDED.duration_secs,
                        file_id = EXCLUDED.file_id,
                        message_id = EXCLUDED.message_id,
                        file_size = EXCLUDED.file_size,
                        created_at = NOW()",
                )
                .bind(user_id)
                .bind(url)
                .bind(title)
                .bind(artist)
                .bind(duration_secs)
                .bind(file_id)
                .bind(message_id)
                .bind(file_size)
                .execute(pg_pool)
                .await
                .context("postgres save_vault_cache_entry")?;
                Ok(())
            }
        }
    }

    pub async fn get_vault_cache_stats(&self, user_id: i64) -> Result<(i64, i64)> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_vault_cache_stats connection")?;
                Ok(db::get_vault_cache_stats(&conn, user_id))
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT COUNT(*)::BIGINT AS count, COALESCE(SUM(file_size), 0)::BIGINT AS total_bytes
                     FROM vault_cache
                     WHERE user_id = $1",
                )
                .bind(user_id)
                .fetch_one(pg_pool)
                .await
                .context("postgres get_vault_cache_stats")?;
                Ok((row.get("count"), row.get("total_bytes")))
            }
        }
    }

    pub async fn clear_vault_cache(&self, user_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite clear_vault_cache connection")?;
                db::clear_vault_cache(&conn, user_id).context("sqlite clear_vault_cache")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("DELETE FROM vault_cache WHERE user_id = $1")
                    .bind(user_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres clear_vault_cache")?;
                Ok(())
            }
        }
    }

    pub async fn upsert_search_session(
        &self,
        user_id: i64,
        query: &str,
        results_json: &str,
        source: &str,
        context_kind: &str,
        playlist_id: Option<i64>,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite upsert_search_session connection")?;
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS search_sessions (
                        user_id INTEGER PRIMARY KEY,
                        query TEXT NOT NULL,
                        results_json TEXT NOT NULL,
                        source TEXT NOT NULL,
                        context_kind TEXT NOT NULL,
                        playlist_id INTEGER,
                        created_at TEXT NOT NULL DEFAULT (datetime('now'))
                    );
                    CREATE INDEX IF NOT EXISTS idx_search_sessions_created_at ON search_sessions(created_at);",
                )
                .context("sqlite ensure search_sessions table")?;
                conn.execute(
                    "INSERT OR REPLACE INTO search_sessions (
                        user_id, query, results_json, source, context_kind, playlist_id, created_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))",
                    rusqlite::params![user_id, query, results_json, source, context_kind, playlist_id],
                )
                .context("sqlite upsert_search_session")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO search_sessions (
                        user_id, query, results_json, source, context_kind, playlist_id, created_at
                     ) VALUES ($1, $2, $3, $4, $5, $6, NOW())
                     ON CONFLICT (user_id) DO UPDATE SET
                        query = EXCLUDED.query,
                        results_json = EXCLUDED.results_json,
                        source = EXCLUDED.source,
                        context_kind = EXCLUDED.context_kind,
                        playlist_id = EXCLUDED.playlist_id,
                        created_at = NOW()",
                )
                .bind(user_id)
                .bind(query)
                .bind(results_json)
                .bind(source)
                .bind(context_kind)
                .bind(playlist_id)
                .execute(pg_pool)
                .await
                .context("postgres upsert_search_session")?;
                Ok(())
            }
        }
    }

    pub async fn get_search_session(
        &self,
        user_id: i64,
        ttl_secs: i64,
    ) -> Result<Option<(String, String, String, String, Option<i64>)>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_search_session connection")?;
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS search_sessions (
                        user_id INTEGER PRIMARY KEY,
                        query TEXT NOT NULL,
                        results_json TEXT NOT NULL,
                        source TEXT NOT NULL,
                        context_kind TEXT NOT NULL,
                        playlist_id INTEGER,
                        created_at TEXT NOT NULL DEFAULT (datetime('now'))
                    );
                    CREATE INDEX IF NOT EXISTS idx_search_sessions_created_at ON search_sessions(created_at);",
                )
                .context("sqlite ensure search_sessions table")?;
                let row = conn
                    .query_row(
                        "SELECT query, results_json, source, context_kind, playlist_id
                         FROM search_sessions
                         WHERE user_id = ?1
                           AND datetime(created_at, '+' || ?2 || ' seconds') > datetime('now')",
                        rusqlite::params![user_id, ttl_secs],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
                    )
                    .optional()
                    .context("sqlite get_search_session")?;
                Ok(row)
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT query, results_json, source, context_kind, playlist_id
                     FROM search_sessions
                     WHERE user_id = $1
                       AND created_at > NOW() - ($2 * INTERVAL '1 second')",
                )
                .bind(user_id)
                .bind(ttl_secs)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_search_session")?;
                Ok(row.map(|row| {
                    (
                        row.get("query"),
                        row.get("results_json"),
                        row.get("source"),
                        row.get("context_kind"),
                        row.get("playlist_id"),
                    )
                }))
            }
        }
    }

    pub async fn delete_search_session(&self, user_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite delete_search_session connection")?;
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS search_sessions (
                        user_id INTEGER PRIMARY KEY,
                        query TEXT NOT NULL,
                        results_json TEXT NOT NULL,
                        source TEXT NOT NULL,
                        context_kind TEXT NOT NULL,
                        playlist_id INTEGER,
                        created_at TEXT NOT NULL DEFAULT (datetime('now'))
                    );
                    CREATE INDEX IF NOT EXISTS idx_search_sessions_created_at ON search_sessions(created_at);",
                )
                .context("sqlite ensure search_sessions table")?;
                conn.execute("DELETE FROM search_sessions WHERE user_id = ?1", [user_id])
                    .context("sqlite delete_search_session")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("DELETE FROM search_sessions WHERE user_id = $1")
                    .bind(user_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres delete_search_session")?;
                Ok(())
            }
        }
    }

    pub async fn upsert_prompt_session(
        &self,
        user_id: i64,
        kind: &str,
        payload_json: &str,
        ttl_secs: i64,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite upsert_prompt_session connection")?;
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS prompt_sessions (
                        user_id INTEGER NOT NULL,
                        kind TEXT NOT NULL,
                        payload_json TEXT NOT NULL DEFAULT '',
                        created_at TEXT NOT NULL DEFAULT (datetime('now')),
                        expires_at TEXT NOT NULL,
                        PRIMARY KEY (user_id, kind)
                    );
                    CREATE INDEX IF NOT EXISTS idx_prompt_sessions_expires_at ON prompt_sessions(expires_at);",
                )
                .context("sqlite ensure prompt_sessions table")?;
                conn.execute(
                    "INSERT OR REPLACE INTO prompt_sessions (
                        user_id, kind, payload_json, created_at, expires_at
                     ) VALUES (?1, ?2, ?3, datetime('now'), datetime('now', '+' || ?4 || ' seconds'))",
                    rusqlite::params![user_id, kind, payload_json, ttl_secs],
                )
                .context("sqlite upsert_prompt_session")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO prompt_sessions (
                        user_id, kind, payload_json, created_at, expires_at
                     ) VALUES ($1, $2, $3, NOW(), NOW() + ($4 * INTERVAL '1 second'))
                     ON CONFLICT (user_id, kind) DO UPDATE SET
                        payload_json = EXCLUDED.payload_json,
                        created_at = NOW(),
                        expires_at = EXCLUDED.expires_at",
                )
                .bind(user_id)
                .bind(kind)
                .bind(payload_json)
                .bind(ttl_secs)
                .execute(pg_pool)
                .await
                .context("postgres upsert_prompt_session")?;
                Ok(())
            }
        }
    }

    pub async fn get_prompt_session(&self, user_id: i64, kind: &str) -> Result<Option<String>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_prompt_session connection")?;
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS prompt_sessions (
                        user_id INTEGER NOT NULL,
                        kind TEXT NOT NULL,
                        payload_json TEXT NOT NULL DEFAULT '',
                        created_at TEXT NOT NULL DEFAULT (datetime('now')),
                        expires_at TEXT NOT NULL,
                        PRIMARY KEY (user_id, kind)
                    );
                    CREATE INDEX IF NOT EXISTS idx_prompt_sessions_expires_at ON prompt_sessions(expires_at);",
                )
                .context("sqlite ensure prompt_sessions table")?;
                let row = conn
                    .query_row(
                        "SELECT payload_json
                         FROM prompt_sessions
                         WHERE user_id = ?1
                           AND kind = ?2
                           AND expires_at > datetime('now')",
                        rusqlite::params![user_id, kind],
                        |row| row.get(0),
                    )
                    .optional()
                    .context("sqlite get_prompt_session")?;
                Ok(row)
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT payload_json
                     FROM prompt_sessions
                     WHERE user_id = $1
                       AND kind = $2
                       AND expires_at > NOW()",
                )
                .bind(user_id)
                .bind(kind)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_prompt_session")?;
                Ok(row.map(|row| row.get("payload_json")))
            }
        }
    }

    pub async fn delete_prompt_session(&self, user_id: i64, kind: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite delete_prompt_session connection")?;
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS prompt_sessions (
                        user_id INTEGER NOT NULL,
                        kind TEXT NOT NULL,
                        payload_json TEXT NOT NULL DEFAULT '',
                        created_at TEXT NOT NULL DEFAULT (datetime('now')),
                        expires_at TEXT NOT NULL,
                        PRIMARY KEY (user_id, kind)
                    );
                    CREATE INDEX IF NOT EXISTS idx_prompt_sessions_expires_at ON prompt_sessions(expires_at);",
                )
                .context("sqlite ensure prompt_sessions table")?;
                conn.execute(
                    "DELETE FROM prompt_sessions WHERE user_id = ?1 AND kind = ?2",
                    rusqlite::params![user_id, kind],
                )
                .context("sqlite delete_prompt_session")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("DELETE FROM prompt_sessions WHERE user_id = $1 AND kind = $2")
                    .bind(user_id)
                    .bind(kind)
                    .execute(pg_pool)
                    .await
                    .context("postgres delete_prompt_session")?;
                Ok(())
            }
        }
    }

    pub async fn upsert_preview_link_message(
        &self,
        user_id: i64,
        url: &str,
        original_message_id: i32,
        ttl_secs: i64,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite upsert_preview_link_message connection")?;
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS preview_contexts (
                        user_id INTEGER NOT NULL,
                        url TEXT NOT NULL,
                        original_message_id INTEGER,
                        time_range_start TEXT,
                        time_range_end TEXT,
                        burn_sub_lang TEXT,
                        created_at TEXT NOT NULL DEFAULT (datetime('now')),
                        expires_at TEXT NOT NULL,
                        PRIMARY KEY (user_id, url)
                    );
                    CREATE INDEX IF NOT EXISTS idx_preview_contexts_expires_at ON preview_contexts(expires_at);",
                )
                .context("sqlite ensure preview_contexts table")?;
                conn.execute(
                    "INSERT INTO preview_contexts (
                        user_id, url, original_message_id, created_at, expires_at
                     ) VALUES (?1, ?2, ?3, datetime('now'), datetime('now', '+' || ?4 || ' seconds'))
                     ON CONFLICT(user_id, url) DO UPDATE SET
                        original_message_id = excluded.original_message_id,
                        expires_at = excluded.expires_at",
                    rusqlite::params![user_id, url, original_message_id, ttl_secs],
                )
                .context("sqlite upsert_preview_link_message")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO preview_contexts (
                        user_id, url, original_message_id, created_at, expires_at
                     ) VALUES ($1, $2, $3, NOW(), NOW() + ($4 * INTERVAL '1 second'))
                     ON CONFLICT (user_id, url) DO UPDATE SET
                        original_message_id = EXCLUDED.original_message_id,
                        expires_at = EXCLUDED.expires_at",
                )
                .bind(user_id)
                .bind(url)
                .bind(original_message_id)
                .bind(ttl_secs)
                .execute(pg_pool)
                .await
                .context("postgres upsert_preview_link_message")?;
                Ok(())
            }
        }
    }

    pub async fn upsert_preview_time_range(
        &self,
        user_id: i64,
        url: &str,
        start: &str,
        end: &str,
        ttl_secs: i64,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite upsert_preview_time_range connection")?;
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS preview_contexts (
                        user_id INTEGER NOT NULL,
                        url TEXT NOT NULL,
                        original_message_id INTEGER,
                        time_range_start TEXT,
                        time_range_end TEXT,
                        burn_sub_lang TEXT,
                        created_at TEXT NOT NULL DEFAULT (datetime('now')),
                        expires_at TEXT NOT NULL,
                        PRIMARY KEY (user_id, url)
                    );
                    CREATE INDEX IF NOT EXISTS idx_preview_contexts_expires_at ON preview_contexts(expires_at);",
                )
                .context("sqlite ensure preview_contexts table")?;
                conn.execute(
                    "INSERT INTO preview_contexts (
                        user_id, url, time_range_start, time_range_end, created_at, expires_at
                     ) VALUES (?1, ?2, ?3, ?4, datetime('now'), datetime('now', '+' || ?5 || ' seconds'))
                     ON CONFLICT(user_id, url) DO UPDATE SET
                        time_range_start = excluded.time_range_start,
                        time_range_end = excluded.time_range_end,
                        expires_at = excluded.expires_at",
                    rusqlite::params![user_id, url, start, end, ttl_secs],
                )
                .context("sqlite upsert_preview_time_range")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO preview_contexts (
                        user_id, url, time_range_start, time_range_end, created_at, expires_at
                     ) VALUES ($1, $2, $3, $4, NOW(), NOW() + ($5 * INTERVAL '1 second'))
                     ON CONFLICT (user_id, url) DO UPDATE SET
                        time_range_start = EXCLUDED.time_range_start,
                        time_range_end = EXCLUDED.time_range_end,
                        expires_at = EXCLUDED.expires_at",
                )
                .bind(user_id)
                .bind(url)
                .bind(start)
                .bind(end)
                .bind(ttl_secs)
                .execute(pg_pool)
                .await
                .context("postgres upsert_preview_time_range")?;
                Ok(())
            }
        }
    }

    pub async fn set_preview_burn_sub_lang(
        &self,
        user_id: i64,
        url: &str,
        burn_sub_lang: Option<&str>,
        ttl_secs: i64,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite set_preview_burn_sub_lang connection")?;
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS preview_contexts (
                        user_id INTEGER NOT NULL,
                        url TEXT NOT NULL,
                        original_message_id INTEGER,
                        time_range_start TEXT,
                        time_range_end TEXT,
                        burn_sub_lang TEXT,
                        created_at TEXT NOT NULL DEFAULT (datetime('now')),
                        expires_at TEXT NOT NULL,
                        PRIMARY KEY (user_id, url)
                    );
                    CREATE INDEX IF NOT EXISTS idx_preview_contexts_expires_at ON preview_contexts(expires_at);",
                )
                .context("sqlite ensure preview_contexts table")?;
                conn.execute(
                    "INSERT INTO preview_contexts (
                        user_id, url, burn_sub_lang, created_at, expires_at
                     ) VALUES (?1, ?2, ?3, datetime('now'), datetime('now', '+' || ?4 || ' seconds'))
                     ON CONFLICT(user_id, url) DO UPDATE SET
                        burn_sub_lang = excluded.burn_sub_lang,
                        expires_at = excluded.expires_at",
                    rusqlite::params![user_id, url, burn_sub_lang, ttl_secs],
                )
                .context("sqlite set_preview_burn_sub_lang")?;
                Ok(())
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO preview_contexts (
                        user_id, url, burn_sub_lang, created_at, expires_at
                     ) VALUES ($1, $2, $3, NOW(), NOW() + ($4 * INTERVAL '1 second'))
                     ON CONFLICT (user_id, url) DO UPDATE SET
                        burn_sub_lang = EXCLUDED.burn_sub_lang,
                        expires_at = EXCLUDED.expires_at",
                )
                .bind(user_id)
                .bind(url)
                .bind(burn_sub_lang)
                .bind(ttl_secs)
                .execute(pg_pool)
                .await
                .context("postgres set_preview_burn_sub_lang")?;
                Ok(())
            }
        }
    }

    pub async fn get_preview_context(&self, user_id: i64, url: &str) -> Result<Option<PreviewContext>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_preview_context connection")?;
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS preview_contexts (
                        user_id INTEGER NOT NULL,
                        url TEXT NOT NULL,
                        original_message_id INTEGER,
                        time_range_start TEXT,
                        time_range_end TEXT,
                        burn_sub_lang TEXT,
                        created_at TEXT NOT NULL DEFAULT (datetime('now')),
                        expires_at TEXT NOT NULL,
                        PRIMARY KEY (user_id, url)
                    );
                    CREATE INDEX IF NOT EXISTS idx_preview_contexts_expires_at ON preview_contexts(expires_at);",
                )
                .context("sqlite ensure preview_contexts table")?;
                let row = conn
                    .query_row(
                        "SELECT original_message_id, time_range_start, time_range_end, burn_sub_lang
                         FROM preview_contexts
                         WHERE user_id = ?1
                           AND url = ?2
                           AND expires_at > datetime('now')",
                        rusqlite::params![user_id, url],
                        |row| {
                            let start: Option<String> = row.get(1)?;
                            let end: Option<String> = row.get(2)?;
                            Ok(PreviewContext {
                                original_message_id: row.get(0)?,
                                time_range: match (start, end) {
                                    (Some(start), Some(end)) => Some((start, end)),
                                    _ => None,
                                },
                                burn_sub_lang: row.get(3)?,
                            })
                        },
                    )
                    .optional()
                    .context("sqlite get_preview_context")?;
                Ok(row)
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT original_message_id, time_range_start, time_range_end, burn_sub_lang
                     FROM preview_contexts
                     WHERE user_id = $1
                       AND url = $2
                       AND expires_at > NOW()",
                )
                .bind(user_id)
                .bind(url)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_preview_context")?;
                Ok(row.map(|row| PreviewContext {
                    original_message_id: row.get("original_message_id"),
                    time_range: match (
                        row.get::<Option<String>, _>("time_range_start"),
                        row.get::<Option<String>, _>("time_range_end"),
                    ) {
                        (Some(start), Some(end)) => Some((start, end)),
                        _ => None,
                    },
                    burn_sub_lang: row.get("burn_sub_lang"),
                }))
            }
        }
    }

    pub async fn save_upload(&self, upload: &NewUpload<'_>) -> Result<i64> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite save_upload connection")?;
                uploads::save_upload(&conn, upload)
                    .map_err(anyhow::Error::from)
                    .context("sqlite save_upload")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "INSERT INTO uploads (
                        user_id, original_filename, title, media_type, file_format,
                        file_id, file_unique_id, file_size, duration, width, height,
                        mime_type, message_id, chat_id, thumbnail_file_id
                     ) VALUES (
                        $1, $2, $3, $4, $5,
                        $6, $7, $8, $9, $10, $11,
                        $12, $13, $14, $15
                     )
                     RETURNING id",
                )
                .bind(upload.user_id)
                .bind(upload.original_filename)
                .bind(upload.title)
                .bind(upload.media_type)
                .bind(upload.file_format)
                .bind(upload.file_id)
                .bind(upload.file_unique_id)
                .bind(upload.file_size)
                .bind(upload.duration)
                .bind(upload.width)
                .bind(upload.height)
                .bind(upload.mime_type)
                .bind(upload.message_id)
                .bind(upload.chat_id)
                .bind(upload.thumbnail_file_id)
                .fetch_one(pg_pool)
                .await
                .context("postgres save_upload")?;
                Ok(row.get("id"))
            }
        }
    }

    pub async fn find_duplicate_upload(&self, user_id: i64, file_unique_id: &str) -> Result<Option<UploadEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite find_duplicate_upload connection")?;
                uploads::find_duplicate_upload(&conn, user_id, file_unique_id)
                    .map_err(anyhow::Error::from)
                    .context("sqlite find_duplicate_upload")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT
                        id, user_id, original_filename, title, media_type, file_format,
                        file_id, file_unique_id, file_size, duration, width, height,
                        mime_type, message_id, chat_id, CAST(uploaded_at AS TEXT) AS uploaded_at, thumbnail_file_id
                     FROM uploads
                     WHERE user_id = $1 AND file_unique_id = $2
                     ORDER BY uploaded_at DESC
                     LIMIT 1",
                )
                .bind(user_id)
                .bind(file_unique_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres find_duplicate_upload")?;
                Ok(row.as_ref().map(upload_entry_from_pg_row))
            }
        }
    }

    pub async fn get_upload_by_id(&self, user_id: i64, upload_id: i64) -> Result<Option<UploadEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_upload_by_id connection")?;
                uploads::get_upload_by_id(&conn, user_id, upload_id)
                    .map_err(anyhow::Error::from)
                    .context("sqlite get_upload_by_id")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT
                        id, user_id, original_filename, title, media_type, file_format,
                        file_id, file_unique_id, file_size, duration, width, height,
                        mime_type, message_id, chat_id, CAST(uploaded_at AS TEXT) AS uploaded_at, thumbnail_file_id
                     FROM uploads
                     WHERE id = $1 AND user_id = $2",
                )
                .bind(upload_id)
                .bind(user_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_upload_by_id")?;
                Ok(row.as_ref().map(upload_entry_from_pg_row))
            }
        }
    }

    pub async fn get_uploads_filtered(
        &self,
        user_id: i64,
        media_type_filter: Option<&str>,
        search_text: Option<&str>,
    ) -> Result<Vec<UploadEntry>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_uploads_filtered connection")?;
                uploads::get_uploads_filtered(&conn, user_id, media_type_filter, search_text)
                    .map_err(anyhow::Error::from)
                    .context("sqlite get_uploads_filtered")
            }
            Self::Postgres { pg_pool, .. } => {
                let base = "SELECT
                        id, user_id, original_filename, title, media_type, file_format,
                        file_id, file_unique_id, file_size, duration, width, height,
                        mime_type, message_id, chat_id, CAST(uploaded_at AS TEXT) AS uploaded_at, thumbnail_file_id
                     FROM uploads
                     WHERE user_id = $1";
                let rows = match (media_type_filter, search_text) {
                    (Some(media_type), Some(search)) => {
                        sqlx::query(&format!(
                            "{base} AND media_type = $2 AND title ILIKE $3 ORDER BY uploaded_at DESC"
                        ))
                        .bind(user_id)
                        .bind(media_type)
                        .bind(format!("%{}%", search))
                        .fetch_all(pg_pool)
                        .await
                    }
                    (Some(media_type), None) => {
                        sqlx::query(&format!("{base} AND media_type = $2 ORDER BY uploaded_at DESC"))
                            .bind(user_id)
                            .bind(media_type)
                            .fetch_all(pg_pool)
                            .await
                    }
                    (None, Some(search)) => {
                        sqlx::query(&format!("{base} AND title ILIKE $2 ORDER BY uploaded_at DESC"))
                            .bind(user_id)
                            .bind(format!("%{}%", search))
                            .fetch_all(pg_pool)
                            .await
                    }
                    (None, None) => {
                        sqlx::query(&format!("{base} ORDER BY uploaded_at DESC"))
                            .bind(user_id)
                            .fetch_all(pg_pool)
                            .await
                    }
                }
                .context("postgres get_uploads_filtered")?;
                Ok(rows.iter().map(upload_entry_from_pg_row).collect())
            }
        }
    }

    pub async fn delete_upload(&self, user_id: i64, upload_id: i64) -> Result<bool> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite delete_upload connection")?;
                uploads::delete_upload(&conn, user_id, upload_id)
                    .map_err(anyhow::Error::from)
                    .context("sqlite delete_upload")
            }
            Self::Postgres { pg_pool, .. } => {
                let result = sqlx::query("DELETE FROM uploads WHERE id = $1 AND user_id = $2")
                    .bind(upload_id)
                    .bind(user_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres delete_upload")?;
                Ok(result.rows_affected() > 0)
            }
        }
    }

    async fn get_user_string_setting(
        &self,
        telegram_id: i64,
        sqlite_selector: &str,
        postgres_query: &str,
        default_value: &str,
    ) -> Result<String> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user_string_setting connection")?;
                match sqlite_selector {
                    "language" => db::get_user_language(&conn, telegram_id),
                    "progress_bar_style" => db::get_user_progress_bar_style(&conn, telegram_id),
                    "video_quality" => db::get_user_video_quality(&conn, telegram_id),
                    "audio_bitrate" => db::get_user_audio_bitrate(&conn, telegram_id),
                    _ => Err(rusqlite::Error::InvalidQuery),
                }
                .map_err(anyhow::Error::from)
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(postgres_query)
                    .bind(telegram_id)
                    .fetch_optional(pg_pool)
                    .await
                    .context("postgres get_user_string_setting")?;
                Ok(row
                    .map(|row| row.get::<String, _>(sqlite_selector))
                    .unwrap_or_else(|| default_value.to_string()))
            }
        }
    }

    async fn get_user_i32_setting(
        &self,
        telegram_id: i64,
        sqlite_selector: &str,
        postgres_query: &str,
        default_value: i32,
    ) -> Result<i32> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user_i32_setting connection")?;
                match sqlite_selector {
                    "send_as_document" => db::get_user_send_as_document(&conn, telegram_id),
                    "send_audio_as_document" => db::get_user_send_audio_as_document(&conn, telegram_id),
                    "download_subtitles" => {
                        db::get_user_download_subtitles(&conn, telegram_id).map(|value| value as i32)
                    }
                    "burn_subtitles" => db::get_user_burn_subtitles(&conn, telegram_id).map(|value| value as i32),
                    _ => Err(rusqlite::Error::InvalidQuery),
                }
                .map_err(anyhow::Error::from)
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(postgres_query)
                    .bind(telegram_id)
                    .fetch_optional(pg_pool)
                    .await
                    .context("postgres get_user_i32_setting")?;
                Ok(row
                    .map(|row| row.get::<i32, _>(sqlite_selector))
                    .unwrap_or(default_value))
            }
        }
    }

    async fn set_user_string_setting(
        &self,
        telegram_id: i64,
        sqlite_selector: &str,
        value: &str,
        postgres_query: &str,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite set_user_string_setting connection")?;
                match sqlite_selector {
                    "language" => db::set_user_language(&conn, telegram_id, value),
                    "progress_bar_style" => db::set_user_progress_bar_style(&conn, telegram_id, value),
                    "video_quality" => db::set_user_video_quality(&conn, telegram_id, value),
                    "audio_bitrate" => db::set_user_audio_bitrate(&conn, telegram_id, value),
                    "subtitle_font_size" => db::set_user_subtitle_font_size(&conn, telegram_id, value),
                    "subtitle_text_color" => db::set_user_subtitle_text_color(&conn, telegram_id, value),
                    "subtitle_outline_color" => db::set_user_subtitle_outline_color(&conn, telegram_id, value),
                    "subtitle_position" => db::set_user_subtitle_position(&conn, telegram_id, value),
                    _ => Err(rusqlite::Error::InvalidQuery),
                }
                .map_err(anyhow::Error::from)
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(postgres_query)
                    .bind(telegram_id)
                    .bind(value)
                    .execute(pg_pool)
                    .await
                    .context("postgres set_user_string_setting")?;
                Ok(())
            }
        }
    }

    async fn set_user_i32_setting(
        &self,
        telegram_id: i64,
        sqlite_selector: &str,
        value: i32,
        postgres_query: &str,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite set_user_i32_setting connection")?;
                match sqlite_selector {
                    "send_as_document" => db::set_user_send_as_document(&conn, telegram_id, value),
                    "send_audio_as_document" => db::set_user_send_audio_as_document(&conn, telegram_id, value),
                    "burn_subtitles" => db::set_user_burn_subtitles(&conn, telegram_id, value != 0),
                    "subtitle_outline_width" => db::set_user_subtitle_outline_width(&conn, telegram_id, value),
                    "subtitle_shadow" => db::set_user_subtitle_shadow(&conn, telegram_id, value),
                    _ => Err(rusqlite::Error::InvalidQuery),
                }
                .map_err(anyhow::Error::from)
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(postgres_query)
                    .bind(telegram_id)
                    .bind(value)
                    .execute(pg_pool)
                    .await
                    .context("postgres set_user_i32_setting")?;
                Ok(())
            }
        }
    }

    async fn run_task_status_update(&self, postgres_query: &str, task_id: &str, worker_id: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite run_task_status_update connection")?;
                if postgres_query.contains("processing") {
                    db::mark_task_processing(&conn, task_id, worker_id).context("sqlite mark_task_processing")
                } else {
                    db::mark_task_uploading(&conn, task_id, worker_id).context("sqlite mark_task_uploading")
                }
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(postgres_query)
                    .bind(task_id)
                    .bind(worker_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres run_task_status_update")?;
                Ok(())
            }
        }
    }

    async fn delete_session_by_user<F>(
        &self,
        user_id: i64,
        table_name: &str,
        sqlite_context: &'static str,
        sqlite_delete: F,
    ) -> Result<()>
    where
        F: FnOnce(&DbConnection, i64) -> rusqlite::Result<()>,
    {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context(sqlite_context)?;
                sqlite_delete(&conn, user_id).map_err(anyhow::Error::from)
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(&format!("DELETE FROM {table_name} WHERE user_id = $1"))
                    .bind(user_id)
                    .execute(pg_pool)
                    .await
                    .with_context(|| format!("postgres delete from {table_name}"))?;
                Ok(())
            }
        }
    }

    async fn upsert_cookies_session(&self, session: &CookiesUploadSession, instagram: bool) -> Result<()> {
        let table_name = if instagram {
            "ig_cookies_upload_sessions"
        } else {
            "cookies_upload_sessions"
        };
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite upsert_cookies_session connection")?;
                if instagram {
                    db::upsert_ig_cookies_upload_session(&conn, session)
                        .context("sqlite upsert_ig_cookies_upload_session")
                } else {
                    db::upsert_cookies_upload_session(&conn, session).context("sqlite upsert_cookies_upload_session")
                }
            }
            Self::Postgres { pg_pool, .. } => {
                let mut tx = pg_pool.begin().await.context("postgres upsert_cookies_session begin")?;
                sqlx::query(&format!("DELETE FROM {table_name} WHERE user_id = $1"))
                    .bind(session.user_id)
                    .execute(&mut *tx)
                    .await
                    .with_context(|| format!("postgres upsert_cookies_session delete from {table_name}"))?;
                sqlx::query(&format!(
                    "INSERT INTO {table_name} (id, user_id, created_at, expires_at) VALUES ($1, $2, $3, $4)"
                ))
                .bind(&session.id)
                .bind(session.user_id)
                .bind(session.created_at)
                .bind(session.expires_at)
                .execute(&mut *tx)
                .await
                .with_context(|| format!("postgres upsert_cookies_session insert into {table_name}"))?;
                tx.commit().await.context("postgres upsert_cookies_session commit")?;
                Ok(())
            }
        }
    }

    async fn get_active_cookies_session(&self, user_id: i64, instagram: bool) -> Result<Option<CookiesUploadSession>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_active_cookies_session connection")?;
                if instagram {
                    db::get_active_ig_cookies_upload_session(&conn, user_id)
                        .context("sqlite get_active_ig_cookies_upload_session")
                } else {
                    db::get_active_cookies_upload_session(&conn, user_id)
                        .context("sqlite get_active_cookies_upload_session")
                }
            }
            Self::Postgres { pg_pool, .. } => {
                let table_name = if instagram {
                    "ig_cookies_upload_sessions"
                } else {
                    "cookies_upload_sessions"
                };
                let row = sqlx::query(&format!(
                    "SELECT * FROM {table_name} WHERE user_id = $1 AND expires_at > NOW() ORDER BY created_at DESC LIMIT 1"
                ))
                .bind(user_id)
                .fetch_optional(pg_pool)
                .await
                .with_context(|| format!("postgres get_active_cookies_session from {table_name}"))?;
                row.map(map_pg_cookies_upload_session).transpose()
            }
        }
    }
}

fn retry_delay_seconds(retry_count: i32) -> i64 {
    let capped = retry_count.clamp(1, 6) as u32;
    30 * 2_i64.pow(capped - 1)
}

fn map_pg_task_queue_entry(row: sqlx::postgres::PgRow) -> Result<TaskQueueEntry> {
    Ok(TaskQueueEntry {
        id: row.get("id"),
        user_id: row.get("user_id"),
        url: row.get("url"),
        message_id: row.get("message_id"),
        format: row.get("format"),
        is_video: row.get::<i32, _>("is_video") == 1,
        video_quality: row.get("video_quality"),
        audio_bitrate: row.get("audio_bitrate"),
        time_range_start: row.get("time_range_start"),
        time_range_end: row.get("time_range_end"),
        carousel_mask: row.get::<Option<i32>, _>("carousel_mask").map(|value| value as u32),
        priority: row.get("priority"),
        status: row.get("status"),
        error_message: row.get("error_message"),
        retry_count: row.get("retry_count"),
        idempotency_key: row.get("idempotency_key"),
        worker_id: row.get("worker_id"),
        leased_at: row.try_get("leased_at").ok(),
        lease_expires_at: row.try_get("lease_expires_at").ok(),
        last_heartbeat_at: row.try_get("last_heartbeat_at").ok(),
        execute_at: row.try_get("execute_at").ok(),
        started_at: row.try_get("started_at").ok(),
        finished_at: row.try_get("finished_at").ok(),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn map_pg_user(row: sqlx::postgres::PgRow) -> Result<User> {
    let plan_raw: String = row.get("plan");
    let plan = Plan::from_str(plan_raw.as_str()).map_err(|err| anyhow!("parse user plan: {}", err))?;
    Ok(User {
        telegram_id: row.get("telegram_id"),
        username: row.get("username"),
        plan,
        download_format: row.get("download_format"),
        download_subtitles: row.get("download_subtitles"),
        video_quality: row.get("video_quality"),
        audio_bitrate: row.get("audio_bitrate"),
        language: row.get("language"),
        send_as_document: row.get("send_as_document"),
        send_audio_as_document: row.get("send_audio_as_document"),
        subscription_expires_at: row.get("subscription_expires_at"),
        telegram_charge_id: row.get("telegram_charge_id"),
        is_recurring: row.get::<i32, _>("is_recurring") != 0,
        burn_subtitles: row.get("burn_subtitles"),
        progress_bar_style: row.get("progress_bar_style"),
        is_blocked: row.get::<i32, _>("is_blocked") != 0,
    })
}

fn map_pg_subtitle_style(row: sqlx::postgres::PgRow) -> SubtitleStyle {
    SubtitleStyle {
        font_size: row.get("subtitle_font_size"),
        text_color: row.get("subtitle_text_color"),
        outline_color: row.get("subtitle_outline_color"),
        outline_width: row.get("subtitle_outline_width"),
        shadow: row.get("subtitle_shadow"),
        position: row.get("subtitle_position"),
    }
}

fn map_pg_playlist(row: sqlx::postgres::PgRow) -> Result<Playlist> {
    Ok(Playlist {
        id: row.get("id"),
        user_id: row.get("user_id"),
        name: row.get("name"),
        description: row.get("description"),
        is_public: row.get::<i32, _>("is_public") != 0,
        share_token: row.get("share_token"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn map_pg_playlist_item(row: sqlx::postgres::PgRow) -> Result<PlaylistItem> {
    Ok(PlaylistItem {
        id: row.get("id"),
        playlist_id: row.get("playlist_id"),
        position: row.get("position"),
        download_history_id: row.get("download_history_id"),
        title: row.get("title"),
        artist: row.get("artist"),
        url: row.get("url"),
        duration_secs: row.get("duration_secs"),
        file_id: row.get("file_id"),
        source: row.get("source"),
        added_at: row.get("added_at"),
    })
}

fn map_pg_synced_playlist(row: sqlx::postgres::PgRow) -> Result<SyncedPlaylist> {
    Ok(SyncedPlaylist {
        id: row.get("id"),
        user_id: row.get("user_id"),
        name: row.get("name"),
        description: row.get("description"),
        source_url: row.get("source_url"),
        source_platform: row.get("source_platform"),
        track_count: row.get("track_count"),
        matched_count: row.get("matched_count"),
        not_found_count: row.get("not_found_count"),
        sync_enabled: row.get::<i32, _>("sync_enabled") != 0,
        last_synced_at: row.get("last_synced_at"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn map_pg_synced_track(row: sqlx::postgres::PgRow) -> Result<SyncedTrack> {
    Ok(SyncedTrack {
        id: row.get("id"),
        playlist_id: row.get("playlist_id"),
        position: row.get("position"),
        title: row.get("title"),
        artist: row.get("artist"),
        duration_secs: row.get("duration_secs"),
        external_id: row.get("external_id"),
        source_url: row.get("source_url"),
        resolved_url: row.get("resolved_url"),
        import_status: row.get("import_status"),
        file_id: row.get("file_id"),
        added_at: row.get("added_at"),
    })
}

fn map_pg_audio_effect_session(row: sqlx::postgres::PgRow) -> Result<AudioEffectSession> {
    Ok(AudioEffectSession {
        id: row.get("id"),
        user_id: row.get("user_id"),
        original_file_path: row.get("original_file_path"),
        current_file_path: row.get("current_file_path"),
        telegram_file_id: row.get("telegram_file_id"),
        original_message_id: row.get("original_message_id"),
        title: row.get("title"),
        duration: row.get::<i64, _>("duration") as u32,
        pitch_semitones: row.get::<i16, _>("pitch_semitones") as i8,
        tempo_factor: row.get::<f64, _>("tempo_factor") as f32,
        bass_gain_db: row.get::<i16, _>("bass_gain_db") as i8,
        morph_profile: MorphProfile::parse(row.get::<String, _>("morph_profile").as_str()),
        version: row.get::<i64, _>("version") as u32,
        processing: row.get::<i32, _>("processing") != 0,
        created_at: row.get("created_at"),
        expires_at: row.get("expires_at"),
    })
}

fn map_pg_audio_cut_session(row: sqlx::postgres::PgRow) -> Result<AudioCutSession> {
    Ok(AudioCutSession {
        id: row.get("id"),
        user_id: row.get("user_id"),
        audio_session_id: row.get("audio_session_id"),
        created_at: row.get("created_at"),
        expires_at: row.get("expires_at"),
    })
}

fn map_pg_video_clip_session(row: sqlx::postgres::PgRow) -> Result<VideoClipSession> {
    Ok(VideoClipSession {
        id: row.get("id"),
        user_id: row.get("user_id"),
        source_download_id: row.get("source_download_id"),
        source_kind: row.get("source_kind"),
        source_id: row.get("source_id"),
        original_url: row.get("original_url"),
        output_kind: row.get("output_kind"),
        created_at: row.get("created_at"),
        expires_at: row.get("expires_at"),
        subtitle_lang: row.get("subtitle_lang"),
    })
}

fn map_pg_cookies_upload_session(row: sqlx::postgres::PgRow) -> Result<CookiesUploadSession> {
    Ok(CookiesUploadSession {
        id: row.get("id"),
        user_id: row.get("user_id"),
        created_at: row.get("created_at"),
        expires_at: row.get("expires_at"),
    })
}

fn parse_json_value(value: Option<String>) -> Option<JsonValue> {
    value.and_then(|raw| serde_json::from_str(&raw).ok())
}

fn map_pg_content_subscription(row: sqlx::postgres::PgRow) -> Result<ContentSubscriptionRecord> {
    Ok(ContentSubscriptionRecord {
        id: row.get("id"),
        user_id: row.get("user_id"),
        source_type: row.get("source_type"),
        source_id: row.get("source_id"),
        display_name: row.get("display_name"),
        watch_mask: row.get::<i32, _>("watch_mask") as u32,
        last_seen_state: parse_json_value(row.get("last_seen_state")),
        source_meta: parse_json_value(row.get("source_meta")),
        is_active: row.get::<i32, _>("is_active") != 0,
        last_checked_at: row.get("last_checked_at"),
        last_error: row.get("last_error"),
        consecutive_errors: row.get::<i32, _>("consecutive_errors") as u32,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn group_content_subscriptions(all_subs: Vec<ContentSubscriptionRecord>) -> Vec<ContentSourceGroup> {
    let mut groups: Vec<ContentSourceGroup> = Vec::new();
    for sub in all_subs {
        if let Some(group) = groups
            .iter_mut()
            .find(|group| group.source_type == sub.source_type && group.source_id == sub.source_id)
        {
            group.combined_mask |= sub.watch_mask;
            group.subscriptions.push(sub);
        } else {
            let combined_mask = sub.watch_mask;
            groups.push(ContentSourceGroup {
                source_type: sub.source_type.clone(),
                source_id: sub.source_id.clone(),
                combined_mask,
                subscriptions: vec![sub],
            });
        }
    }
    groups
}

fn sqlite_parse_content_subscription_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ContentSubscriptionRecord> {
    let last_seen_state: Option<String> = row.get(7)?;
    let source_meta: Option<String> = row.get(8)?;
    Ok(ContentSubscriptionRecord {
        id: row.get(0)?,
        user_id: row.get(1)?,
        source_type: row.get(2)?,
        source_id: row.get(3)?,
        display_name: row.get(4)?,
        watch_mask: row.get::<_, u32>(5)?,
        last_seen_state: parse_json_value(last_seen_state),
        source_meta: parse_json_value(source_meta),
        is_active: row.get::<_, i32>(6)? != 0,
        last_checked_at: row.get(9)?,
        last_error: row.get(10)?,
        consecutive_errors: row.get::<_, u32>(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

fn sqlite_upsert_content_subscription(
    conn: &DbConnection,
    user_id: i64,
    source_type: &str,
    source_id: &str,
    display_name: &str,
    watch_mask: u32,
    source_meta: Option<&JsonValue>,
) -> rusqlite::Result<i64> {
    let meta_json = source_meta.map(|value| value.to_string());
    conn.execute(
        "INSERT INTO content_subscriptions (user_id, source_type, source_id, display_name, watch_mask, source_meta, is_active, consecutive_errors, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, 0, CURRENT_TIMESTAMP)
         ON CONFLICT(user_id, source_type, source_id) DO UPDATE SET
           watch_mask = ?5,
           display_name = ?4,
           source_meta = COALESCE(?6, source_meta),
           is_active = 1,
           consecutive_errors = 0,
           last_error = NULL,
           updated_at = CURRENT_TIMESTAMP",
        rusqlite::params![user_id, source_type, source_id, display_name, watch_mask, meta_json],
    )?;
    conn.query_row(
        "SELECT id FROM content_subscriptions WHERE user_id = ?1 AND source_type = ?2 AND source_id = ?3",
        rusqlite::params![user_id, source_type, source_id],
        |row| row.get(0),
    )
}

fn sqlite_get_content_subscription(
    conn: &DbConnection,
    id: i64,
) -> rusqlite::Result<Option<ContentSubscriptionRecord>> {
    conn.query_row(
        "SELECT id, user_id, source_type, source_id, display_name, watch_mask, is_active,
                last_seen_state, source_meta, last_checked_at, last_error, consecutive_errors,
                created_at, updated_at
         FROM content_subscriptions WHERE id = ?1",
        rusqlite::params![id],
        sqlite_parse_content_subscription_row,
    )
    .optional()
}

fn sqlite_get_user_content_subscriptions(
    conn: &DbConnection,
    user_id: i64,
) -> rusqlite::Result<Vec<ContentSubscriptionRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, source_type, source_id, display_name, watch_mask, is_active,
                last_seen_state, source_meta, last_checked_at, last_error, consecutive_errors,
                created_at, updated_at
         FROM content_subscriptions
         WHERE user_id = ?1 AND is_active = 1
         ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map(rusqlite::params![user_id], sqlite_parse_content_subscription_row)?;
    rows.collect()
}

fn sqlite_count_user_content_subscriptions(conn: &DbConnection, user_id: i64) -> rusqlite::Result<u32> {
    conn.query_row(
        "SELECT COUNT(*) FROM content_subscriptions WHERE user_id = ?1 AND is_active = 1",
        rusqlite::params![user_id],
        |row| row.get(0),
    )
}

fn sqlite_has_content_subscription(
    conn: &DbConnection,
    user_id: i64,
    source_type: &str,
    source_id: &str,
) -> rusqlite::Result<Option<ContentSubscriptionRecord>> {
    conn.query_row(
        "SELECT id, user_id, source_type, source_id, display_name, watch_mask, is_active,
                last_seen_state, source_meta, last_checked_at, last_error, consecutive_errors,
                created_at, updated_at
         FROM content_subscriptions
         WHERE user_id = ?1 AND source_type = ?2 AND source_id = ?3",
        rusqlite::params![user_id, source_type, source_id],
        sqlite_parse_content_subscription_row,
    )
    .optional()
}

fn sqlite_deactivate_content_subscription(conn: &DbConnection, id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE content_subscriptions SET is_active = 0, updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
        rusqlite::params![id],
    )?;
    Ok(())
}

fn sqlite_deactivate_all_content_subscriptions_for_user(conn: &DbConnection, user_id: i64) -> rusqlite::Result<u32> {
    Ok(conn.execute(
        "UPDATE content_subscriptions SET is_active = 0, updated_at = CURRENT_TIMESTAMP
         WHERE user_id = ?1 AND is_active = 1",
        rusqlite::params![user_id],
    )? as u32)
}

fn sqlite_update_content_watch_mask(conn: &DbConnection, id: i64, new_mask: u32) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE content_subscriptions SET watch_mask = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
        rusqlite::params![new_mask, id],
    )?;
    Ok(())
}

fn sqlite_get_active_content_source_groups(conn: &DbConnection) -> rusqlite::Result<Vec<ContentSourceGroup>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, source_type, source_id, display_name, watch_mask, is_active,
                last_seen_state, source_meta, last_checked_at, last_error, consecutive_errors,
                created_at, updated_at
         FROM content_subscriptions
         WHERE is_active = 1
         ORDER BY last_checked_at ASC NULLS FIRST, source_type, source_id",
    )?;
    let rows = stmt.query_map([], sqlite_parse_content_subscription_row)?;
    let all_subs: rusqlite::Result<Vec<_>> = rows.collect();
    Ok(group_content_subscriptions(all_subs?))
}

fn sqlite_update_content_check_success(
    conn: &DbConnection,
    source_type: &str,
    source_id: &str,
    new_state: &JsonValue,
    new_meta: Option<&JsonValue>,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE content_subscriptions
         SET last_seen_state = ?1,
             source_meta = COALESCE(?2, source_meta),
             last_checked_at = CURRENT_TIMESTAMP,
             last_error = NULL,
             consecutive_errors = 0,
             updated_at = CURRENT_TIMESTAMP
         WHERE source_type = ?3 AND source_id = ?4 AND is_active = 1",
        rusqlite::params![
            new_state.to_string(),
            new_meta.map(|value| value.to_string()),
            source_type,
            source_id
        ],
    )?;
    Ok(())
}

fn sqlite_update_content_check_error(
    conn: &DbConnection,
    source_type: &str,
    source_id: &str,
    error: &str,
) -> rusqlite::Result<u32> {
    conn.execute(
        "UPDATE content_subscriptions
         SET last_checked_at = CURRENT_TIMESTAMP,
             last_error = ?1,
             consecutive_errors = consecutive_errors + 1,
             updated_at = CURRENT_TIMESTAMP
         WHERE source_type = ?2 AND source_id = ?3 AND is_active = 1",
        rusqlite::params![error, source_type, source_id],
    )?;
    conn.query_row(
        "SELECT COALESCE(MAX(consecutive_errors), 0)
         FROM content_subscriptions
         WHERE source_type = ?1 AND source_id = ?2 AND is_active = 1",
        rusqlite::params![source_type, source_id],
        |row| row.get(0),
    )
}

fn sqlite_auto_disable_errored_content(
    conn: &DbConnection,
    source_type: &str,
    source_id: &str,
    max_errors: u32,
) -> rusqlite::Result<u32> {
    Ok(conn.execute(
        "UPDATE content_subscriptions
         SET is_active = 0, updated_at = CURRENT_TIMESTAMP
         WHERE source_type = ?1 AND source_id = ?2 AND is_active = 1 AND consecutive_errors >= ?3",
        rusqlite::params![source_type, source_id, max_errors],
    )? as u32)
}

fn map_pg_download_history(row: sqlx::postgres::PgRow) -> DownloadHistoryEntry {
    DownloadHistoryEntry {
        id: row.get("id"),
        url: row.get("url"),
        title: row.get("title"),
        format: row.get("format"),
        downloaded_at: row.get("downloaded_at"),
        file_id: row.get("file_id"),
        author: row.get("author"),
        file_size: row.get("file_size"),
        duration: row.get("duration"),
        video_quality: row.get("video_quality"),
        audio_bitrate: row.get("audio_bitrate"),
        bot_api_url: row.get("bot_api_url"),
        bot_api_is_local: row.get("bot_api_is_local"),
        source_id: row.get("source_id"),
        part_index: row.get("part_index"),
        category: row.get("category"),
    }
}

fn map_pg_charge(row: sqlx::postgres::PgRow) -> Result<Charge> {
    let plan = Plan::from_str(&row.get::<String, _>("plan"))
        .map_err(|err| anyhow!("invalid charge plan in postgres: {err}"))?;

    Ok(Charge {
        id: row.get("id"),
        user_id: row.get("user_id"),
        plan,
        telegram_charge_id: row.get("telegram_charge_id"),
        provider_charge_id: row.get("provider_charge_id"),
        currency: row.get("currency"),
        total_amount: row.get("total_amount"),
        invoice_payload: row.get("invoice_payload"),
        is_recurring: row.get::<i32, _>("is_recurring") != 0,
        is_first_recurring: row.get::<i32, _>("is_first_recurring") != 0,
        subscription_expiration_date: row.get("subscription_expiration_date"),
        payment_date: row.get("payment_date"),
        created_at: row.get("created_at"),
    })
}

fn map_pg_sent_file(row: sqlx::postgres::PgRow) -> Result<SentFile> {
    Ok(SentFile {
        id: row.get("id"),
        user_id: row.get("user_id"),
        username: row.get("username"),
        url: row.get("url"),
        title: row.get("title"),
        format: row.get("format"),
        downloaded_at: row.get("downloaded_at"),
        file_id: row.get("file_id"),
        message_id: row.get("message_id"),
        chat_id: row.get("chat_id"),
    })
}

fn map_pg_error_log_entry(row: sqlx::postgres::PgRow) -> ErrorLogEntry {
    ErrorLogEntry {
        id: row.get("id"),
        timestamp: row.get("timestamp"),
        user_id: row.get("user_id"),
        username: row.get("username"),
        error_type: row.get("error_type"),
        error_message: row.get("error_message"),
        url: row.get("url"),
        context: row.get("context"),
        resolved: row.get::<i32, _>("resolved") != 0,
    }
}

fn map_pg_share_page_record(row: sqlx::postgres::PgRow) -> SharePageRecord {
    SharePageRecord {
        id: row.get("id"),
        youtube_url: row.get("youtube_url"),
        title: row.get("title"),
        artist: row.get("artist"),
        thumbnail_url: row.get("thumbnail_url"),
        duration_secs: row.get("duration_secs"),
        streaming_links_json: row.get("streaming_links"),
        created_at: row.get("created_at"),
    }
}

fn map_pg_cut(row: sqlx::postgres::PgRow) -> CutEntry {
    CutEntry {
        id: row.get("id"),
        user_id: row.get("user_id"),
        original_url: row.get("original_url"),
        source_kind: row.get("source_kind"),
        source_id: row.get("source_id"),
        output_kind: row.get("output_kind"),
        segments_json: row.get("segments_json"),
        segments_text: row.get("segments_text"),
        title: row.get("title"),
        created_at: row.get("created_at"),
        file_id: row.get("file_id"),
        file_size: row.get("file_size"),
        duration: row.get("duration"),
        video_quality: row.get("video_quality"),
    }
}
