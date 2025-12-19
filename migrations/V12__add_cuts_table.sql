-- Cuts (video excerpts)
-- Separate entity from download_history, with explicit original source and flexible segment metadata.

CREATE TABLE IF NOT EXISTS cuts (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL,

    -- Original source (must always point to the original video URL)
    original_url TEXT NOT NULL,

    -- Where this cut was produced from: "download" (download_history.id) or "cut" (cuts.id)
    source_kind TEXT NOT NULL,
    source_id INTEGER NOT NULL,

    -- Cut segments metadata (JSON array of {start_secs,end_secs})
    segments_json TEXT NOT NULL,
    segments_text TEXT NOT NULL,

    title TEXT NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,

    file_id TEXT DEFAULT NULL,
    file_size INTEGER DEFAULT NULL,
    duration INTEGER DEFAULT NULL,
    video_quality TEXT DEFAULT NULL
);

CREATE INDEX IF NOT EXISTS idx_cuts_user_id ON cuts(user_id);
CREATE INDEX IF NOT EXISTS idx_cuts_created_at ON cuts(created_at DESC);

-- Extend video clip sessions to support cutting both downloads and cuts
ALTER TABLE video_clip_sessions ADD COLUMN source_kind TEXT DEFAULT 'download';
ALTER TABLE video_clip_sessions ADD COLUMN source_id INTEGER DEFAULT NULL;
ALTER TABLE video_clip_sessions ADD COLUMN original_url TEXT DEFAULT NULL;

