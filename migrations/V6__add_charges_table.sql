-- Create charges table to track all Telegram Stars payments
-- This table stores complete payment information for accounting and reconciliation
CREATE TABLE IF NOT EXISTS charges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    plan TEXT NOT NULL,
    telegram_charge_id TEXT NOT NULL UNIQUE,
    provider_charge_id TEXT,
    currency TEXT NOT NULL,
    total_amount INTEGER NOT NULL,
    invoice_payload TEXT NOT NULL,
    is_recurring INTEGER NOT NULL DEFAULT 0,
    is_first_recurring INTEGER NOT NULL DEFAULT 0,
    subscription_expiration_date DATETIME,
    payment_date DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users(telegram_id)
);

-- Indexes for efficient queries
CREATE INDEX IF NOT EXISTS idx_charges_user_id ON charges(user_id);
CREATE INDEX IF NOT EXISTS idx_charges_telegram_charge_id ON charges(telegram_charge_id);
CREATE INDEX IF NOT EXISTS idx_charges_plan ON charges(plan);
CREATE INDEX IF NOT EXISTS idx_charges_payment_date ON charges(payment_date);
CREATE INDEX IF NOT EXISTS idx_charges_is_recurring ON charges(is_recurring);
