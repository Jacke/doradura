pub(super) const POSTGRES_BOOTSTRAP_SQL: &str = r#"
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
