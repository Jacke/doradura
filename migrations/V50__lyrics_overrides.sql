-- Canonical lyrics overrides: maps a media source (a YouTube/other video) to a
-- user-corrected lyrics snapshot. Global by design — one correction for a video
-- benefits everyone who downloads it. Keyed by the *canonicalized* source URL.
--
-- When a download has lyrics enabled, the fetch path consults this table first
-- and uses the stored snapshot instead of the (possibly wrong) auto-match.
CREATE TABLE IF NOT EXISTS lyrics_overrides (
    source_key   TEXT PRIMARY KEY,   -- canonicalize_url(source video URL)
    provider     TEXT    NOT NULL,   -- 'lrclib' | 'genius' (where the fix came from)
    source_url   TEXT    NOT NULL,   -- the lyrics URL the user supplied
    artist       TEXT,
    title        TEXT,
    lyrics_text  TEXT    NOT NULL,   -- resolved lyrics snapshot (plain text)
    corrected_by INTEGER,            -- telegram user_id who set the correction (audit)
    created_at   TEXT    NOT NULL,
    updated_at   TEXT    NOT NULL
);
