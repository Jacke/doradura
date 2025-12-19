-- Extend cuts with output kind
-- output_kind:
-- - clip: regular MP4 excerpt
-- - video_note: Telegram video note (circle)

ALTER TABLE cuts ADD COLUMN output_kind TEXT DEFAULT 'clip';

