# /info Command

## Description
`/info` returns detailed information about available download formats without actually downloading the file.

## Usage
```
/info <URL>
```

## Example
```
/info https://www.youtube.com/watch?v=dQw4w9WgXcQ
```

## What it shows
### 📹 Video formats (MP4)
- Qualities: 1080p, 720p, 480p, 360p (if available)
- File size (MB)
- Resolution (e.g., 1920x1080)

### 🎧 Audio format (MP3)
- Bitrate: 320 kbps
- File size (MB)

### ⏱ Extra info
- Title
- Duration (MM:SS)
- Download instructions

## Technical details
- File: `src/telegram/commands.rs`
- Function: `handle_info_command()`
- Uses: `get_preview_metadata()` from `src/telegram/preview.rs`
- Async handling
