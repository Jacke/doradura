CREATE TABLE IF NOT EXISTS bot_assets (
    key TEXT PRIMARY KEY,
    file_id TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
