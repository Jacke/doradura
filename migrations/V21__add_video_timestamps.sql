-- Video timestamps extracted from URLs, chapters, and descriptions
-- Used to suggest clip segments when user creates video notes (circles)

CREATE TABLE IF NOT EXISTS video_timestamps (
    id INTEGER PRIMARY KEY,
    download_id INTEGER NOT NULL,
    source TEXT NOT NULL,          -- 'url', 'chapter', 'description'
    time_seconds INTEGER NOT NULL, -- Start time in seconds
    end_seconds INTEGER,           -- End time (only for chapters)
    label TEXT,                    -- Title/description of the timestamp
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (download_id) REFERENCES download_history(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_video_timestamps_download_id ON video_timestamps(download_id);
