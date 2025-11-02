-- Create the users table if it does not exist
CREATE TABLE IF NOT EXISTS users (
    id INTEGER PRIMARY KEY,
    telegram_id INTEGER UNIQUE NOT NULL,
    username TEXT,
    plan TEXT DEFAULT 'free',
    download_format TEXT DEFAULT 'mp3',
    download_subtitles INTEGER DEFAULT 0
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
