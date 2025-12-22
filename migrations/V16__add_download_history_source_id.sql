-- Add source_id and part_index to link split video parts
ALTER TABLE download_history ADD COLUMN source_id INTEGER DEFAULT NULL;
ALTER TABLE download_history ADD COLUMN part_index INTEGER DEFAULT NULL;

-- Index for faster lookups by source_id
CREATE INDEX IF NOT EXISTS idx_download_history_source_id ON download_history(source_id);
