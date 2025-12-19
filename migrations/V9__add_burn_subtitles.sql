-- Migration V9: Add burn_subtitles option
-- This migration adds a flag for burning (hardcoding) subtitles into video

-- Add burn_subtitles column to users table
-- 0 = disabled (subtitles as separate file)
-- 1 = enabled (subtitles burned into video)
ALTER TABLE users ADD COLUMN burn_subtitles INTEGER DEFAULT 0;
