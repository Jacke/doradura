-- V49: Silent downloads + MOTD digest.
--
-- silent_downloads: per-user toggle. When 1, downloads run at Low priority,
-- emit no queue-position / progress messages, and are delivered with
-- disable_notification. Flipped from the Settings menu and the preview card.
--
-- silent_digest: completed (or failed) silent downloads awaiting a one-time
-- MOTD recap on the user's next interaction with the bot. Rows are marked
-- shown = 1 once recapped, then pruned periodically.

ALTER TABLE users ADD COLUMN silent_downloads INTEGER DEFAULT 0;

CREATE TABLE IF NOT EXISTS silent_digest (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id      INTEGER NOT NULL,
    title        TEXT,
    format       TEXT,
    status       TEXT NOT NULL DEFAULT 'done',   -- 'done' | 'failed'
    completed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    shown        INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_silent_digest_user_shown ON silent_digest(user_id, shown);
