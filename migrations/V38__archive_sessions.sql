-- Archive sessions: allows users to bundle downloaded files into ZIP archives
CREATE TABLE archive_sessions (
    id          TEXT PRIMARY KEY,
    user_id     INTEGER NOT NULL,
    status      TEXT NOT NULL DEFAULT 'selecting',
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at  TEXT NOT NULL
);

CREATE TABLE archive_session_items (
    session_id  TEXT NOT NULL REFERENCES archive_sessions(id) ON DELETE CASCADE,
    download_id INTEGER NOT NULL,
    UNIQUE(session_id, download_id)
);

CREATE INDEX idx_archive_sessions_user ON archive_sessions(user_id, status);
CREATE INDEX idx_archive_items_session ON archive_session_items(session_id);
