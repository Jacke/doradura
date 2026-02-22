CREATE TABLE IF NOT EXISTS share_pages (
  id TEXT PRIMARY KEY,
  youtube_url TEXT NOT NULL,
  title TEXT NOT NULL,
  artist TEXT,
  thumbnail_url TEXT,
  duration_secs INTEGER,
  streaming_links TEXT,  -- JSON: {"spotify":"url","appleMusic":"url",...}
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
