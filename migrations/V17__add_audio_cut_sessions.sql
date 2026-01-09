-- Audio cut sessions
-- Stores a temporary "waiting for time range" state after user selects ✂️ Cut Audio.

CREATE TABLE IF NOT EXISTS audio_cut_sessions (
    id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    audio_session_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_audio_cut_sessions_user_id ON audio_cut_sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_audio_cut_sessions_expires_at ON audio_cut_sessions(expires_at);
