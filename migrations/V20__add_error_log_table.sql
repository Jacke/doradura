-- Error log table for tracking errors with user context
-- Used for monitoring, debugging, and periodic admin reports

CREATE TABLE IF NOT EXISTS error_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
    user_id INTEGER,                      -- telegram_id
    username TEXT,                        -- @username if available
    error_type TEXT NOT NULL,             -- 'download_failed', 'mtproto_error', 'file_too_large', etc.
    error_message TEXT NOT NULL,          -- Human-readable error message
    url TEXT,                             -- URL if applicable
    context TEXT,                         -- JSON with additional data (file_id, format, etc.)
    resolved INTEGER DEFAULT 0            -- Whether error was resolved/retried successfully
);

-- Index for time-based queries (recent errors)
CREATE INDEX IF NOT EXISTS idx_error_log_timestamp ON error_log(timestamp DESC);

-- Index for user-specific error queries
CREATE INDEX IF NOT EXISTS idx_error_log_user_id ON error_log(user_id);

-- Index for error type filtering
CREATE INDEX IF NOT EXISTS idx_error_log_type ON error_log(error_type);

-- Composite index for period stats queries
CREATE INDEX IF NOT EXISTS idx_error_log_period ON error_log(timestamp, error_type);
