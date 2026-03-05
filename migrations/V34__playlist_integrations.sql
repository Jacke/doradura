-- Synced playlists from external platforms (Spotify, SoundCloud, Yandex Music, YouTube)
CREATE TABLE synced_playlists (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    source_url TEXT NOT NULL,
    source_platform TEXT NOT NULL,  -- 'spotify', 'soundcloud', 'yandex_music', 'youtube'
    track_count INTEGER NOT NULL DEFAULT 0,
    matched_count INTEGER NOT NULL DEFAULT 0,
    not_found_count INTEGER NOT NULL DEFAULT 0,
    sync_enabled INTEGER NOT NULL DEFAULT 0,
    last_synced_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Individual tracks within synced playlists
CREATE TABLE synced_tracks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    playlist_id INTEGER NOT NULL REFERENCES synced_playlists(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    title TEXT NOT NULL,
    artist TEXT,
    duration_secs INTEGER,
    external_id TEXT,       -- spotify:track:xxx, sc:xxx, ym:xxx
    source_url TEXT,        -- original platform URL
    resolved_url TEXT,      -- YouTube/SC downloadable URL
    import_status TEXT NOT NULL DEFAULT 'pending',  -- 'matched', 'not_found', 'pending'
    file_id TEXT,           -- cached Telegram file_id after download
    added_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_synced_playlists_user ON synced_playlists(user_id);
CREATE INDEX idx_synced_tracks_playlist ON synced_tracks(playlist_id);
CREATE INDEX idx_synced_tracks_external ON synced_tracks(playlist_id, external_id);
