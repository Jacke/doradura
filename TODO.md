# TODO / Work Plan

## Update Rules (LLM-friendly)
- Use only `- [ ]` (not started), `- [~]` (in progress), `- [x]` (done).
- Dates in `YYYY-MM-DD` format; only add them in "Decision Log" and "Blockers".
- Do not delete others' entries; move tasks between sections without rephrasing.
- Keep it short: each task <= 140 characters, no Markdown links.
- If you need a new section, add it at the end of the file.

## Summary
- Weekly focus: ...
- Key outcomes: ...
- Risks/blockers: ...

## Now
- [ ] ...

## Next
- [ ] ...

## Backlog
- [ ] ...

## Done
- [x] Add /info command to show available formats for URLs
- [x] Display video formats (MP4) with quality and file sizes
- [x] Display audio format (MP3) information
- [x] Show resolution and bitrate details
- [x] Add extractor-args to fix YouTube SABR streaming and nsig extraction issues
- [x] Add video title as caption when sending videos
- [x] Add detailed format logging to /info command for debugging
- [x] Fix video format detection to include video-only formats with audio size estimation

## Decision Log

- 2025-12-07 - Added youtube:player_client=default,web_safari,web_embedded to handle SABR streaming and nsig extraction failures

## Blockers
- 2024-... - ...

## Ideas and Notes
- ...
