-- Create the users table if it does not exist
CREATE TABLE IF NOT EXISTS users (
    id INTEGER PRIMARY KEY,
    telegram_id INTEGER UNIQUE NOT NULL,
    username TEXT,
    plan TEXT DEFAULT 'free'
);

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
