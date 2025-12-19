-- Create dedicated subscriptions table to store all subscription-related data
CREATE TABLE IF NOT EXISTS subscriptions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL UNIQUE,
    plan TEXT NOT NULL DEFAULT 'free',
    expires_at DATETIME DEFAULT NULL,
    telegram_charge_id TEXT DEFAULT NULL,
    is_recurring INTEGER NOT NULL DEFAULT 0,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users(telegram_id)
);

CREATE INDEX IF NOT EXISTS idx_subscriptions_user_id ON subscriptions(user_id);
CREATE INDEX IF NOT EXISTS idx_subscriptions_plan ON subscriptions(plan);

-- Backfill subscriptions from existing user data
INSERT OR IGNORE INTO subscriptions (user_id, plan, expires_at, telegram_charge_id, is_recurring)
SELECT
    telegram_id,
    plan,
    subscription_expires_at,
    telegram_charge_id,
    COALESCE(is_recurring, 0)
FROM users
WHERE telegram_id IS NOT NULL;
