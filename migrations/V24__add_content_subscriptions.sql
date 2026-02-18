CREATE TABLE IF NOT EXISTS content_subscriptions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    source_type TEXT NOT NULL,
    source_id TEXT NOT NULL,
    display_name TEXT NOT NULL DEFAULT '',
    watch_mask INTEGER NOT NULL DEFAULT 3,
    last_seen_state TEXT DEFAULT NULL,
    source_meta TEXT DEFAULT NULL,
    is_active INTEGER NOT NULL DEFAULT 1,
    last_checked_at DATETIME DEFAULT NULL,
    last_error TEXT DEFAULT NULL,
    consecutive_errors INTEGER NOT NULL DEFAULT 0,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(user_id, source_type, source_id),
    FOREIGN KEY (user_id) REFERENCES users(telegram_id)
);

CREATE INDEX IF NOT EXISTS idx_content_subs_active ON content_subscriptions(is_active, last_checked_at);
CREATE INDEX IF NOT EXISTS idx_content_subs_user ON content_subscriptions(user_id, is_active);
CREATE INDEX IF NOT EXISTS idx_content_subs_source ON content_subscriptions(source_type, source_id, is_active);
