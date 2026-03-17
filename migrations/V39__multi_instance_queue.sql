ALTER TABLE task_queue ADD COLUMN idempotency_key TEXT;
ALTER TABLE task_queue ADD COLUMN worker_id TEXT;
ALTER TABLE task_queue ADD COLUMN leased_at DATETIME;
ALTER TABLE task_queue ADD COLUMN lease_expires_at DATETIME;
ALTER TABLE task_queue ADD COLUMN last_heartbeat_at DATETIME;
ALTER TABLE task_queue ADD COLUMN execute_at DATETIME;
ALTER TABLE task_queue ADD COLUMN started_at DATETIME;
ALTER TABLE task_queue ADD COLUMN finished_at DATETIME;
ALTER TABLE task_queue ADD COLUMN message_id INTEGER;
ALTER TABLE task_queue ADD COLUMN time_range_start TEXT;
ALTER TABLE task_queue ADD COLUMN time_range_end TEXT;
ALTER TABLE task_queue ADD COLUMN carousel_mask INTEGER;

CREATE INDEX IF NOT EXISTS idx_task_queue_runnable
    ON task_queue(status, priority DESC, created_at ASC);
CREATE INDEX IF NOT EXISTS idx_task_queue_lease_expiry
    ON task_queue(status, lease_expires_at);
CREATE INDEX IF NOT EXISTS idx_task_queue_user_pending
    ON task_queue(user_id, status, created_at ASC);
CREATE UNIQUE INDEX IF NOT EXISTS idx_task_queue_active_idempotency
    ON task_queue(idempotency_key)
    WHERE idempotency_key IS NOT NULL
      AND status IN ('pending', 'leased', 'processing', 'uploading');

CREATE TABLE IF NOT EXISTS processed_updates (
    bot_id INTEGER NOT NULL,
    update_id INTEGER NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (bot_id, update_id)
);

CREATE INDEX IF NOT EXISTS idx_processed_updates_created_at
    ON processed_updates(created_at);
