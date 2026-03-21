CREATE TABLE IF NOT EXISTS admin_audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    admin_id INTEGER NOT NULL,
    action TEXT NOT NULL,          -- 'plan_change', 'block', 'unblock', 'broadcast', 'resolve_error', 'ack_alert', 'feedback_status', 'retry_task', 'cancel_task', 'user_settings'
    target_type TEXT NOT NULL,     -- 'user', 'error', 'alert', 'feedback', 'task', 'broadcast'
    target_id TEXT NOT NULL,       -- user telegram_id, error id, etc.
    details TEXT,                  -- JSON with additional context
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_admin_audit_log_created ON admin_audit_log(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_admin_audit_log_admin ON admin_audit_log(admin_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_admin_audit_log_action ON admin_audit_log(action);
