-- Add new columns to download_history table for enhanced metadata
-- This enables the /downloads command to show rich file information

-- Add author field (separated from title for better search)
ALTER TABLE download_history ADD COLUMN author TEXT;

-- Add file size in bytes
ALTER TABLE download_history ADD COLUMN file_size INTEGER;

-- Add duration in seconds
ALTER TABLE download_history ADD COLUMN duration INTEGER;

-- Add video quality (e.g., '1080p', '720p', '480p')
ALTER TABLE download_history ADD COLUMN video_quality TEXT;

-- Add audio bitrate (e.g., '320k', '256k', '192k')
ALTER TABLE download_history ADD COLUMN audio_bitrate TEXT;

-- Add indices for better search and filter performance
CREATE INDEX IF NOT EXISTS idx_download_history_title ON download_history(title);
CREATE INDEX IF NOT EXISTS idx_download_history_author ON download_history(author);
