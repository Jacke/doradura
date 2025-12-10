# Black Screen Fix (v2)

## Context
A follow-up to address remaining black-screen cases after the initial fix.

## Changes
- **Format fallback:** if a combined video+audio format is unavailable, automatically pick the best video-only plus best audio and merge.
- **Validation before send:** ensure the merged file actually contains a video stream; fail fast with a user-friendly message otherwise.
- **Quality selection:** map requested quality to the closest available format to avoid unsupported resolutions that lead to empty streams.
- **Logging:** capture yt-dlp format selection and ffmpeg merge output for easier debugging.

## How to test
1. Download several problem URLs (shorts, standard videos, different qualities).
2. Confirm playback works in Telegram and locally.
3. Review logs for format selection and ffmpeg results.

## Recovery steps if seen again
- Update yt-dlp/ffmpeg.
- Switch to another quality (e.g., 720p or "best").
- Re-run with cookies to access restricted formats.

This iteration should cover the remaining black-screen edge cases by enforcing valid streams and safer fallbacks.
