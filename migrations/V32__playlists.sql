-- Playlist & Player Mode tables

CREATE TABLE playlists (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id     INTEGER NOT NULL,
    name        TEXT    NOT NULL,
    description TEXT,
    is_public   INTEGER NOT NULL DEFAULT 0,
    share_token TEXT    UNIQUE,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX idx_playlists_user_id ON playlists(user_id);

CREATE TABLE playlist_items (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    playlist_id         INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    position            INTEGER NOT NULL DEFAULT 0,
    download_history_id INTEGER,
    title               TEXT NOT NULL,
    artist              TEXT,
    url                 TEXT NOT NULL,
    duration_secs       INTEGER,
    file_id             TEXT,
    source              TEXT NOT NULL DEFAULT 'youtube',
    added_at            TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX idx_playlist_items_playlist ON playlist_items(playlist_id, position);

CREATE TABLE player_sessions (
    user_id           INTEGER PRIMARY KEY,
    playlist_id       INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    current_position  INTEGER NOT NULL DEFAULT 0,
    is_shuffle        INTEGER NOT NULL DEFAULT 0,
    player_message_id INTEGER,
    updated_at        TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE search_cache (
    query_key   TEXT PRIMARY KEY,
    results_json TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
