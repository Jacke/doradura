-- V48: Global cache of already-downloaded files indexed by (url, format).
--
-- Feeds the alpha.29 "Guest Bots" feature: when someone @-mentions the bot
-- in a group it isn't a member of with a URL that ANY user has previously
-- downloaded, we can answer the guest_query with an InlineQueryResultCached*
-- using the stored file_id — sub-second viral delivery, no re-download.
--
-- LRU semantics: every hit bumps `hits` and `last_used`; periodic cleanup
-- removes rows that are old AND under-hit. See background_tasks::cleanup_popular_files.

CREATE TABLE IF NOT EXISTS popular_files (
    url TEXT NOT NULL,
    format TEXT NOT NULL,           -- mp3, mp4, m4r, ringtone, gif, video_note, cut
    file_id TEXT NOT NULL,
    title TEXT,                     -- best-effort display title for the inline result
    author TEXT,                    -- best-effort author for caption
    duration INTEGER,               -- seconds, for audio/video display
    file_size INTEGER,              -- bytes
    hits INTEGER NOT NULL DEFAULT 1,
    first_seen TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    last_used TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (url, format)
);

CREATE INDEX IF NOT EXISTS idx_popular_files_last_used ON popular_files(last_used DESC);
CREATE INDEX IF NOT EXISTS idx_popular_files_hits ON popular_files(hits DESC);
