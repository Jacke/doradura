# Download Progress Bar Fix

## Problem
The progress bar stayed on "⏳ Starting download..." and skipped many percentage updates.

## Cause
Progress updates were only sent when the percentage was a multiple of 5:
```rust
if progress_info.percent % 5 == 0 && progress_info.percent != last_progress {
    // update UI
}
```
Any values like 3%, 8%, 13%, 27% were ignored.

## Solution

1. **Update logic (`src/downloader.rs`)**
   - Track the last shown percent and update whenever it increases by at least 1%.
   - Always emit 100% at completion.

2. **Minimum interval guard**
   - Keep a small debounce to avoid flooding the API while still showing smooth progress.

3. **Logging**
   - Added debug logs for parsed progress from yt-dlp to aid troubleshooting.

## Result
- Progress now advances smoothly for both audio and video downloads.
- 100% is guaranteed at the end.
- No stuck "starting" state.
