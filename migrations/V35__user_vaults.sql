-- User Vault: private Telegram channel as persistent file storage

CREATE TABLE user_vaults (
    user_id     INTEGER PRIMARY KEY,
    channel_id  INTEGER NOT NULL,
    channel_title TEXT,
    is_active   INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE vault_cache (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id       INTEGER NOT NULL,
    url           TEXT NOT NULL,
    title         TEXT,
    artist        TEXT,
    duration_secs INTEGER,
    file_id       TEXT NOT NULL,
    message_id    INTEGER,
    file_size     INTEGER,
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(user_id, url)
);

CREATE INDEX idx_vault_cache_lookup ON vault_cache(user_id, url);
