CREATE TABLE IF NOT EXISTS user_categories (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id    INTEGER NOT NULL,
    name       TEXT    NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(user_id, name)
);

ALTER TABLE download_history ADD COLUMN category TEXT;

CREATE TABLE IF NOT EXISTS new_category_sessions (
    user_id     INTEGER PRIMARY KEY,
    download_id INTEGER NOT NULL,
    created_at  DATETIME DEFAULT CURRENT_TIMESTAMP
);
