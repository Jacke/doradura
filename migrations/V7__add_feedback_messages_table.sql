-- Create feedback_messages table to store all user feedback
-- This table helps track and manage user feedback for better customer support
CREATE TABLE IF NOT EXISTS feedback_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    username TEXT,
    first_name TEXT NOT NULL,
    message TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'new',
    admin_reply TEXT,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    replied_at DATETIME,
    FOREIGN KEY (user_id) REFERENCES users(telegram_id)
);

-- Indexes for efficient queries
CREATE INDEX IF NOT EXISTS idx_feedback_user_id ON feedback_messages(user_id);
CREATE INDEX IF NOT EXISTS idx_feedback_status ON feedback_messages(status);
CREATE INDEX IF NOT EXISTS idx_feedback_created_at ON feedback_messages(created_at);
