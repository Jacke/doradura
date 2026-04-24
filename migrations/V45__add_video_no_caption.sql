-- Migration V45: Add video_no_caption option
-- This migration adds a per-user flag to suppress the caption on sent videos

-- Add video_no_caption column to users table
-- 0 = default (caption with artist/title/signature)
-- 1 = enabled (video sent without caption)
ALTER TABLE users ADD COLUMN video_no_caption INTEGER DEFAULT 0;
