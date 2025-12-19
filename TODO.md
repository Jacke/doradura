# TODO / Work Plan

## Update Rules (LLM-friendly)
- Use only `- [ ]` (not started), `- [~]` (in progress), `- [x]` (done).
- Dates in `YYYY-MM-DD` format; only add them in "Decision Log" and "Blockers".
- Do not delete others' entries; move tasks between sections without rephrasing.
- Keep it short: each task <= 140 characters, no Markdown links.
- If you need a new section, add it at the end of the file.

## Now
- [ ] ...

## Next
- [ ] When user selects a file (from /downloads) he can see the info of the file and perform modification, such as increase speed, or cut the file to a specific duration. it also could cut multiple sections of the file.
- [ ]

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
- [x] Update format selectors to use bestvideo+bestaudio for proper video/audio merging
