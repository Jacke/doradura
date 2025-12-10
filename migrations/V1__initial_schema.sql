-- Initial schema (without language column; see V2 for language).

-- Users and subscription plans
CREATE TABLE IF NOT EXISTS users (
    id INTEGER PRIMARY KEY,
    telegram_id INTEGER UNIQUE NOT NULL,
    username TEXT,
    plan TEXT DEFAULT 'free',
    download_format TEXT DEFAULT 'mp3',
    download_subtitles INTEGER DEFAULT 0,
    video_quality TEXT DEFAULT 'best',
    audio_bitrate TEXT DEFAULT '320k',
    send_as_document INTEGER DEFAULT 0,
    send_audio_as_document INTEGER DEFAULT 0,
    subscription_expires_at DATETIME DEFAULT NULL,
    telegram_charge_id TEXT DEFAULT NULL
);

CREATE TABLE IF NOT EXISTS subscription_plans (
    id INTEGER PRIMARY KEY,
    name TEXT UNIQUE NOT NULL,
    description TEXT
);

INSERT INTO subscription_plans (name, description)
    SELECT 'free', 'Free plan with limited functionality'
    WHERE NOT EXISTS (SELECT 1 FROM subscription_plans WHERE name = 'free');

INSERT INTO subscription_plans (name, description)
    SELECT 'paid', 'Paid plan with full functionality'
    WHERE NOT EXISTS (SELECT 1 FROM subscription_plans WHERE name = 'paid');

-- Request history
CREATE TABLE IF NOT EXISTS request_history (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL,
    request_text TEXT NOT NULL,
    timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users(id)
);

-- Download history
CREATE TABLE IF NOT EXISTS download_history (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL,
    url TEXT NOT NULL,
    title TEXT NOT NULL,
    format TEXT NOT NULL,
    downloaded_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    file_id TEXT DEFAULT NULL,
    FOREIGN KEY (user_id) REFERENCES users(telegram_id)
);

CREATE INDEX IF NOT EXISTS idx_download_history_user_id ON download_history(user_id);
CREATE INDEX IF NOT EXISTS idx_download_history_downloaded_at ON download_history(downloaded_at DESC);

-- Task queue
CREATE TABLE IF NOT EXISTS task_queue (
    id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    url TEXT NOT NULL,
    format TEXT NOT NULL,
    is_video INTEGER NOT NULL DEFAULT 0,
    video_quality TEXT,
    audio_bitrate TEXT,
    priority INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'pending',
    error_message TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users(telegram_id)
);

CREATE INDEX IF NOT EXISTS idx_task_queue_status ON task_queue(status);
CREATE INDEX IF NOT EXISTS idx_task_queue_user_id ON task_queue(user_id);
CREATE INDEX IF NOT EXISTS idx_task_queue_created_at ON task_queue(created_at);

-- URL cache
CREATE TABLE IF NOT EXISTS url_cache (
    id TEXT PRIMARY KEY,
    url TEXT NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    expires_at DATETIME NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_url_cache_expires_at ON url_cache(expires_at);

-- Audio effect sessions
CREATE TABLE IF NOT EXISTS audio_effect_sessions (
    id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    original_file_path TEXT NOT NULL,
    current_file_path TEXT NOT NULL,
    telegram_file_id TEXT,
    original_message_id INTEGER NOT NULL,
    title TEXT NOT NULL,
    duration INTEGER NOT NULL,
    pitch_semitones INTEGER DEFAULT 0,
    tempo_factor REAL DEFAULT 1.0,
    bass_gain_db INTEGER DEFAULT 0,
    morph_profile TEXT DEFAULT 'none',
    version INTEGER DEFAULT 0,
    processing INTEGER DEFAULT 0,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    expires_at DATETIME NOT NULL,
    FOREIGN KEY (user_id) REFERENCES users(telegram_id)
);

CREATE INDEX IF NOT EXISTS idx_audio_sessions_user_id ON audio_effect_sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_audio_sessions_expires_at ON audio_effect_sessions(expires_at);
CREATE INDEX IF NOT EXISTS idx_audio_sessions_msg_id ON audio_effect_sessions(original_message_id);
