# yt-dlp Update Quick Reference

## One-Line Commands

### Check Version
```bash
cargo run -- update-ytdlp --check
```

### Update if Needed
```bash
cargo run -- update-ytdlp
```

### Force Update
```bash
cargo run -- update-ytdlp --force
```

### Full Update Cycle (Recommended)
```bash
# Force update + restart bot
cargo run -- update-ytdlp --force && cargo run -- run
```

## Troubleshooting 403 Errors

### Step 1: Update yt-dlp
```bash
cargo run -- update-ytdlp --force
```

### Step 2: Update Cookies
```bash
# If using file-based cookies
export YOUTUBE_COOKIES_PATH=/path/to/new_cookies.txt
cargo run -- run

# Or use browser extraction
export YTDL_COOKIES_BROWSER=chrome
cargo run -- run
```

### Step 3: Test Download
```bash
# Use a known working YouTube URL and test
yt-dlp "https://www.youtube.com/watch?v=dQw4w9WgXcQ" -j
```

## In Production

### Set Up Daily Auto-Update (Cron)
```bash
# Edit crontab
crontab -e

# Add this line (runs at 2 AM daily)
0 2 * * * cd /path/to/doradura && cargo run -- update-ytdlp --force 2>&1 | logger -t ytdlp-update
```

### Check Update Logs
```bash
tail -f logs/bot.log | grep -i ytdlp
```

### Validate Installation After Update
```bash
# Check version
yt-dlp --version

# List available extractors (should be 700+)
yt-dlp --list-extractors | wc -l
```

## Environment Variables

```bash
# Specify yt-dlp binary location
export YTDL_BIN=/usr/local/bin/yt-dlp

# Use Python cookies extraction
export YTDL_COOKIES_BROWSER=chrome

# Or use file-based cookies
export YOUTUBE_COOKIES_PATH=/home/user/cookies.txt

# Set update timeout (seconds)
export YTDLP_UPDATE_TIMEOUT=300
```

## Error Messages & Fixes

| Error | Cause | Fix |
|-------|-------|-----|
| `HTTP Error 403: Forbidden` | Outdated yt-dlp or blocked requests | Update yt-dlp, refresh cookies |
| `Unable to extract video id` | Signature extraction failed | Force update yt-dlp |
| `[youtube] Skipping fragment` | Fragment download failed | Already handled by retry logic (10 retries) |
| `pip command not found` | Python/pip not installed | Install `python3-pip` |
| `Permission denied` | Insufficient permissions | Use `--user` flag or `sudo` |

## Status Checks

### Verify yt-dlp Health
```bash
# Check version
cargo run -- update-ytdlp --check

# Test extraction (dry run, no download)
yt-dlp --dump-json "https://www.youtube.com/watch?v=dQw4w9WgXcQ" | jq '.id'

# Check supported extractors
yt-dlp --list-extractors | head -20
```

### Check Bot Fragment Retry Configuration
Current settings in `downloader.rs`:
- **Concurrent fragments**: 3 (reduced for safety)
- **Fragment retries**: 10 (automatic recovery)
- **Socket timeout**: 30 seconds
- **HTTP chunk size**: 10 MB
- **Request delay**: 1 ms

These ensure most transient 403 errors are automatically recovered.

## When to Update

| Scenario | Action |
|----------|--------|
| Bot startup | Automatic check (updates if needed) |
| 403 errors increasing | Force update immediately |
| Weekly maintenance | Run `update-ytdlp` |
| Before deploying | Run `update-ytdlp --force` |
| After 403 in logs | Run `update-ytdlp --force` |
| Unknown errors | Run `update-ytdlp --check` then update |

## Development vs Production

### Development
```bash
# Regular check during testing
cargo run -- update-ytdlp --check

# Update when testing fails
cargo run -- update-ytdlp --force
```

### Production
```bash
# Automated daily updates (cron)
0 2 * * * cargo run -- update-ytdlp --force

# Monitor logs for errors
grep -i "403\|fragment\|error" logs/bot.log

# React to errors
cargo run -- update-ytdlp --force && systemctl restart doradura
```

## Related Commands

```bash
# View all available subcommands
cargo run -- --help

# Show yt-dlp help
yt-dlp --help | less

# Show yt-dlp version details
yt-dlp --version

# Check specific URL support
yt-dlp --dump-json "https://youtube.com/watch?v=..." | jq '.id'
```

## Getting Help

Check detailed documentation:
```bash
# Full update guide
cat docs/YTDLP_UPDATE_GUIDE.md

# YouTube error handling
cat docs/FIX_YOUTUBE_ERRORS.md

# Cookie management
cat docs/YOUTUBE_COOKIES.md

# General troubleshooting
cat docs/TROUBLESHOOTING.md
```
