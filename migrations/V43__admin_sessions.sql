-- Admin web session store.
--
-- Replaces the previous deterministic sha256(user_id:bot_token) cookie scheme
-- with random 32-byte session tokens stored hashed in the database.
--
-- Properties:
-- - token_hash is sha256(raw_token); the raw token is never persisted.
-- - expires_at lets verify_admin reject stale cookies server-side.
-- - Each login creates a new row; logout deletes it (real revocation).

CREATE TABLE IF NOT EXISTS admin_sessions (
    token_hash BLOB PRIMARY KEY,
    admin_id   INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at TEXT NOT NULL,
    last_seen  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    user_agent TEXT,
    ip         TEXT
);

CREATE INDEX IF NOT EXISTS idx_admin_sessions_admin ON admin_sessions(admin_id);
CREATE INDEX IF NOT EXISTS idx_admin_sessions_expires ON admin_sessions(expires_at);
