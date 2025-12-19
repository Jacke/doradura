-- Add is_recurring flag to track recurring subscriptions
ALTER TABLE users ADD COLUMN is_recurring INTEGER DEFAULT 0;
