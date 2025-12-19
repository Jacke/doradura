-- Video clip sessions
-- Stores a temporary "waiting for time range" state after user selects ✂️ Clip in /downloads.

CREATE TABLE IF NOT EXISTS video_clip_sessions (
    id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    source_download_id INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_video_clip_sessions_user_id ON video_clip_sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_video_clip_sessions_expires_at ON video_clip_sessions(expires_at);

