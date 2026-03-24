-- Add repeat mode and last track index to player sessions
-- repeat_mode: 0 = off, 1 = repeat all, 2 = repeat one
ALTER TABLE player_sessions ADD COLUMN IF NOT EXISTS repeat_mode INTEGER NOT NULL DEFAULT 0;
ALTER TABLE player_sessions ADD COLUMN IF NOT EXISTS last_track_index INTEGER;
