-- Stores fetched and parsed lyrics for a short session window.
-- Linked to an audio_effect_session so we can re-use artist/title without re-fetching.
CREATE TABLE IF NOT EXISTS lyrics_sessions (
    id          TEXT    PRIMARY KEY,
    user_id     INTEGER NOT NULL,
    artist      TEXT    NOT NULL,
    title       TEXT    NOT NULL,
    sections_json TEXT  NOT NULL,      -- JSON array of {name, lines[]}
    has_structure INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT    NOT NULL,
    expires_at  TEXT    NOT NULL       -- 24 h from creation
);

CREATE INDEX IF NOT EXISTS idx_lyrics_sessions_expires ON lyrics_sessions(expires_at);
