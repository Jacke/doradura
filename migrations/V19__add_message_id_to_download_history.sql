-- Add message_id to download_history for MTProto file_reference refresh
-- This allows fetching fresh file_reference via messages.getMessages

ALTER TABLE download_history ADD COLUMN message_id INTEGER DEFAULT NULL;
ALTER TABLE download_history ADD COLUMN chat_id INTEGER DEFAULT NULL;

-- Index for quick lookup by message_id
CREATE INDEX IF NOT EXISTS idx_download_history_message_id ON download_history(message_id);

-- Also add to cuts table
ALTER TABLE cuts ADD COLUMN message_id INTEGER DEFAULT NULL;
ALTER TABLE cuts ADD COLUMN chat_id INTEGER DEFAULT NULL;

CREATE INDEX IF NOT EXISTS idx_cuts_message_id ON cuts(message_id);
