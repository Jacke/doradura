-- Record which Bot API server was used when persisting download_history entries.
-- This helps debug whether large Telegram files can be downloaded later (local Bot API server vs api.telegram.org).

ALTER TABLE download_history ADD COLUMN bot_api_url TEXT DEFAULT NULL;
ALTER TABLE download_history ADD COLUMN bot_api_is_local INTEGER DEFAULT 0;

