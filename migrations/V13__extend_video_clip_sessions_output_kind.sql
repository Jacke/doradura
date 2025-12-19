-- Extend video_clip_sessions with output_kind
-- output_kind:
-- - cut: produce a regular MP4 cut and save into cuts
-- - video_note: produce a Telegram video note (circle) and save into cuts

ALTER TABLE video_clip_sessions ADD COLUMN output_kind TEXT DEFAULT 'cut';

