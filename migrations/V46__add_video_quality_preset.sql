-- Migration V46: Add video_quality_preset (codec-aware encoding tier)
--
-- New per-user setting that controls how high-res (1440p+) video sources
-- are processed after download:
--   'balanced'    — medium / CRF 17 / AAC 192k (the v0.45.3 fallback)
--   'transparent' — slow / CRF 14 / AAC 320k (~99 VMAF, visually identical)
--   'master'      — veryslow / CRF 12 / AAC 320k (~99.5 VMAF, near-master)
--   'lossless'    — no recode (AV1 sent as document, VP9 → mkv remux)
--
-- Default 'master' matches the v0.46.0 "best bot" identity. H.264 sources
-- never get recoded regardless of preset (always remuxed to mp4 stream-copy).

ALTER TABLE users ADD COLUMN video_quality_preset TEXT DEFAULT 'master';
