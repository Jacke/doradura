-- Index for cross-user file_id dedup lookup (find_cached_file_id)
-- Covers: WHERE url = ? AND format = ? AND bot_api_is_local = ? ORDER BY downloaded_at DESC
CREATE INDEX IF NOT EXISTS idx_download_history_url_format_api
ON download_history(url, format, bot_api_is_local, downloaded_at DESC);

-- Composite index for task recovery query (get_and_reset_recoverable_tasks)
-- Covers: WHERE status = ? AND created_at > ? ORDER BY priority DESC, created_at ASC
CREATE INDEX IF NOT EXISTS idx_task_queue_status_created
ON task_queue(status, created_at);
