# Test Summary

A concise record of the yt-dlp integration tests and fixes.

## Scope
- Environment diagnostics (yt-dlp, ffmpeg, cookies).
- Metadata retrieval.
- Audio download.
- Error handling for invalid URLs.
- Different quality/bitrate combinations.

## Results
- Diagnostics pass on the reference environment.
- Metadata and audio download tests succeed with fresh cookies.
- Invalid URL tests correctly return errors.
- Quality-selection tests confirm proper mapping to available formats.

## Known caveats
- Tests requiring internet are marked `#[ignore]`; run them manually with `--ignored`.
- Cookies must be present for YouTube; stale cookies will cause failures.

## How to rerun
```bash
cargo test --test ytdlp_integration_test test_full_diagnostics -- --nocapture
cargo test --test ytdlp_integration_test --ignored -- --nocapture --test-threads=1
```

## Follow-up items
- Add more edge cases (long videos, region restrictions).
- Consider automating cookie refresh for CI-like environments.
- Collect metrics on download duration during tests for baseline tracking.
