# Fix: Black Screen on Video Download

## Symptom
Downloaded videos sometimes contained audio but displayed a black screen.

## Root causes considered
- Wrong codec/container combination.
- Missing video stream (audio-only format selected).
- Telegram transcoding quirks.

## Fixes applied
1. **Force proper format selection**
   - Use combined formats when available; avoid audio-only for video requests.
   - Validate that the selected format includes a video stream.
2. **Explicit codecs/containers**
   - Prefer `mp4` outputs compatible with Telegram.
3. **ffmpeg safeguards**
   - Verify streams during merge; log stderr for diagnostics.
4. **User feedback**
   - Clear error message if only audio is available or if merge fails.

## Verification
- Test downloads for multiple URLs with and without cookies.
- Confirm video plays correctly in Telegram and local players.

## Tips if it reappears
- Update yt-dlp and ffmpeg to the latest versions.
- Try a different video quality/format.
- Check logs for the selected format and ffmpeg output.
