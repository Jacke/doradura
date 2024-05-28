-- Create the users table
CREATE TABLE users (
    id INTEGER PRIMARY KEY,
    telegram_id INTEGER UNIQUE NOT NULL,
    username TEXT,
    plan TEXT DEFAULT 'free'
);

-- Create the subscription_plans table
CREATE TABLE subscription_plans (
    id INTEGER PRIMARY KEY,
    name TEXT UNIQUE NOT NULL,
    description TEXT
);

-- Insert default subscription plans
INSERT INTO subscription_plans (name, description) VALUES
    ('free', 'Free plan with limited functionality'),
    ('paid', 'Paid plan with full functionality');

-- Create the request_history table
CREATE TABLE request_history (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL,
    request_text TEXT NOT NULL,
    timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users(id)
);