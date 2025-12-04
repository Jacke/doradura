-- Create the users table if it does not exist
CREATE TABLE IF NOT EXISTS users (
    id INTEGER PRIMARY KEY,
    telegram_id INTEGER UNIQUE NOT NULL,
    username TEXT,
    plan TEXT DEFAULT 'free',
    download_format TEXT DEFAULT 'mp3',
    download_subtitles INTEGER DEFAULT 0,
    video_quality TEXT DEFAULT 'best',
    audio_bitrate TEXT DEFAULT '320k',
    subscription_expires_at DATETIME DEFAULT NULL
);

-- Add missing columns to existing users table (if they don't exist)
-- SQLite doesn't support IF NOT EXISTS for ALTER TABLE, so we need to use a workaround
-- We'll use a pragma to check if the column exists, but SQLite doesn't support that either
-- Instead, we'll catch errors and ignore them, or use a different approach

-- Add missing columns to existing users table
-- Note: SQLite doesn't support IF NOT EXISTS for ALTER TABLE
-- We'll handle errors in the application code if columns already exist
-- SQLite will return an error if the column already exists, which we'll ignore

-- Create the subscription_plans table if it does not exist
CREATE TABLE IF NOT EXISTS subscription_plans (
    id INTEGER PRIMARY KEY,
    name TEXT UNIQUE NOT NULL,
    description TEXT
);

-- Insert default subscription plans if they do not exist
INSERT INTO subscription_plans (name, description) 
    SELECT 'free', 'Free plan with limited functionality'
    WHERE NOT EXISTS (SELECT 1 FROM subscription_plans WHERE name = 'free');

INSERT INTO subscription_plans (name, description) 
    SELECT 'paid', 'Paid plan with full functionality'
    WHERE NOT EXISTS (SELECT 1 FROM subscription_plans WHERE name = 'paid');

-- Create the request_history table if it does not exist
CREATE TABLE IF NOT EXISTS request_history (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL,
    request_text TEXT NOT NULL,
    timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users(id)
);

-- Create the download_history table if it does not exist
CREATE TABLE IF NOT EXISTS download_history (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL,
    url TEXT NOT NULL,
    title TEXT NOT NULL,
    format TEXT NOT NULL,
    downloaded_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users(telegram_id)
);

-- Create index for faster queries
CREATE INDEX IF NOT EXISTS idx_download_history_user_id ON download_history(user_id);
CREATE INDEX IF NOT EXISTS idx_download_history_downloaded_at ON download_history(downloaded_at DESC);

-- Create the task_queue table for persistent task tracking
-- This table ensures all tasks are processed even after bot restarts
CREATE TABLE IF NOT EXISTS task_queue (
    id TEXT PRIMARY KEY,  -- UUID of the task
    user_id INTEGER NOT NULL,
    url TEXT NOT NULL,
    format TEXT NOT NULL,
    is_video INTEGER NOT NULL DEFAULT 0,
    video_quality TEXT,
    audio_bitrate TEXT,
    priority INTEGER NOT NULL DEFAULT 0,  -- 0=Low, 1=Medium, 2=High
    status TEXT NOT NULL DEFAULT 'pending',  -- pending, processing, failed, completed
    error_message TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users(telegram_id)
);

-- Create indexes for faster queries
CREATE INDEX IF NOT EXISTS idx_task_queue_status ON task_queue(status);
CREATE INDEX IF NOT EXISTS idx_task_queue_user_id ON task_queue(user_id);
CREATE INDEX IF NOT EXISTS idx_task_queue_created_at ON task_queue(created_at);

-- Create the url_cache table for storing URL mappings (survives bot restarts)
CREATE TABLE IF NOT EXISTS url_cache (
    id TEXT PRIMARY KEY,  -- Short hash ID (12 chars)
    url TEXT NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    expires_at DATETIME NOT NULL  -- TTL: 30 minutes from creation
);

-- Create index for faster lookups and cleanup
CREATE INDEX IF NOT EXISTS idx_url_cache_expires_at ON url_cache(expires_at);

-- Add subscription_expires_at column to existing users table if it doesn't exist
-- This will fail silently if the column already exists, which is fine
-- Note: SQLite doesn't support IF NOT EXISTS for ALTER TABLE ADD COLUMN
