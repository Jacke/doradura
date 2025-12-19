-- Migration V8: Add Analytics Tables
-- This migration adds tables for storing analytics data and aggregates

-- Hourly metric aggregates for fast querying and historical data
CREATE TABLE IF NOT EXISTS metric_aggregates (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    metric_name TEXT NOT NULL,           -- e.g., 'doradura_download_duration_seconds'
    metric_type TEXT NOT NULL,           -- 'counter', 'gauge', 'histogram'
    labels TEXT NOT NULL,                -- JSON string with label key-value pairs
    value REAL NOT NULL,                 -- Metric value
    timestamp DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    period TEXT NOT NULL DEFAULT 'hourly'  -- 'hourly', 'daily', 'monthly'
);

-- Index for fast time-based queries
CREATE INDEX IF NOT EXISTS idx_metric_aggregates_name_time
    ON metric_aggregates(metric_name, timestamp DESC);

-- Index for period-based queries
CREATE INDEX IF NOT EXISTS idx_metric_aggregates_period
    ON metric_aggregates(period, timestamp DESC);

-- Index for composite queries
CREATE INDEX IF NOT EXISTS idx_metric_aggregates_composite
    ON metric_aggregates(metric_name, period, timestamp DESC);

-- Alert history for tracking triggered alerts
CREATE TABLE IF NOT EXISTS alert_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    alert_type TEXT NOT NULL,            -- 'high_error_rate', 'queue_backup', 'payment_failure', etc.
    severity TEXT NOT NULL,               -- 'critical', 'warning', 'info'
    message TEXT NOT NULL,                -- Alert message sent to admin
    metadata TEXT,                        -- JSON with additional context (error counts, queue depth, etc.)
    triggered_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    resolved_at DATETIME,                 -- When the alert condition was resolved
    acknowledged BOOLEAN DEFAULT 0,       -- Whether admin acknowledged the alert
    acknowledged_at DATETIME              -- When admin acknowledged
);

-- Index for alert type queries
CREATE INDEX IF NOT EXISTS idx_alert_history_type ON alert_history(alert_type);

-- Index for time-based queries
CREATE INDEX IF NOT EXISTS idx_alert_history_triggered ON alert_history(triggered_at DESC);

-- Index for unresolved alerts
CREATE INDEX IF NOT EXISTS idx_alert_history_unresolved
    ON alert_history(alert_type, resolved_at) WHERE resolved_at IS NULL;

-- User activity tracking for DAU/MAU calculation
CREATE TABLE IF NOT EXISTS user_activity (
    user_id INTEGER NOT NULL,
    activity_date DATE NOT NULL,         -- Date of activity (YYYY-MM-DD)
    command_count INTEGER DEFAULT 0,     -- Number of commands executed
    download_count INTEGER DEFAULT 0,    -- Number of downloads requested
    last_activity_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (user_id, activity_date),
    FOREIGN KEY (user_id) REFERENCES users(telegram_id) ON DELETE CASCADE
);

-- Index for date-based DAU/MAU queries
CREATE INDEX IF NOT EXISTS idx_user_activity_date ON user_activity(activity_date DESC);

-- Index for user-specific activity
CREATE INDEX IF NOT EXISTS idx_user_activity_user ON user_activity(user_id, activity_date DESC);

-- Optimize existing tables for analytics queries
-- These indexes improve performance of analytics queries on existing tables

-- Optimize download_history for format/time analytics
CREATE INDEX IF NOT EXISTS idx_download_history_format_date
    ON download_history(format, downloaded_at DESC);

-- Optimize download_history for user analytics
CREATE INDEX IF NOT EXISTS idx_download_history_user_date
    ON download_history(user_id, downloaded_at DESC);

-- Optimize charges for revenue analytics
CREATE INDEX IF NOT EXISTS idx_charges_date_plan
    ON charges(payment_date DESC, plan);

-- Optimize charges for recurring subscription analysis
CREATE INDEX IF NOT EXISTS idx_charges_recurring
    ON charges(is_recurring, payment_date DESC);

-- Optimize task_queue for queue depth analytics
CREATE INDEX IF NOT EXISTS idx_task_queue_status_priority
    ON task_queue(status, priority, created_at DESC);
