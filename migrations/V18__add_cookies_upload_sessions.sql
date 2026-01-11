-- Cookies upload sessions
-- Stores a temporary "waiting for cookies file" state after admin calls /update_cookies

CREATE TABLE IF NOT EXISTS cookies_upload_sessions (
    id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_cookies_upload_sessions_user_id ON cookies_upload_sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_cookies_upload_sessions_expires_at ON cookies_upload_sessions(expires_at);
