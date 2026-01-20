# yt-dlp Update Guide

## Overview

This guide explains how to manage yt-dlp updates in the Doradura bot. Regular updates are critical for maintaining YouTube video downloads because YouTube frequently changes its extraction methods, requiring yt-dlp to adapt.

## Why Update yt-dlp?

YouTube actively works against video downloaders by:
- Changing player algorithms and JavaScript signatures
- Updating authentication mechanisms
- Blocking outdated user agents
- Rotating API endpoints and rate limits

Without regular yt-dlp updates, you'll encounter errors like:
- `HTTP Error 403: Forbidden` - Fragment download failures
- `Unable to extract video id` - Signature extraction failed
- `Sign in to confirm you're not a bot` - Authentication failures
- `Video unavailable` - Format or regional restrictions

## Automatic Updates at Startup

The bot automatically checks and updates yt-dlp when started:

```bash
# Normal startup (auto-checks for updates)
cargo run -- run

# Staging mode (auto-checks for updates)
cargo run -- run-staging

# With cookies (auto-checks for updates)
cargo run -- run-with-cookies --cookies /path/to/cookies.txt
```

## Manual Update Commands

### Check Current Version

```bash
cargo run -- update-ytdlp --check
```

Output:
```
yt-dlp version: 2024.12.16
```

### Update (Only if Needed)

```bash
cargo run -- update-ytdlp
```

This command:
- Checks if updates are available
- Only updates if a newer version exists
- Works with both system and pip installations

Output:
```
✅ yt-dlp is already up to date
```

Or:
```
✅ yt-dlp updated successfully
```

### Force Update

```bash
cargo run -- update-ytdlp --force
```

This command:
- Always attempts to update, even if current version is recent
- Useful if you suspect installation corruption
- Ignores the version check

Output:
```
Force updating yt-dlp to the latest version...
✅ yt-dlp updated successfully
```

## Supported Installation Methods

### 1. System Package (Recommended for Servers)

Installation:
```bash
# macOS
brew install yt-dlp

# Ubuntu/Debian
sudo apt-get install yt-dlp

# Other Linux
sudo dnf install yt-dlp  # Fedora
sudo pacman -S yt-dlp   # Arch
```

Update mechanism: `yt-dlp -U`

### 2. Python pip (Flexible)

Installation:
```bash
pip install yt-dlp
# or
pip3 install yt-dlp
```

Update mechanism: `pip install --upgrade yt-dlp` or `pip3 install --upgrade yt-dlp`

The bot supports both `pip` and `pip3` and tries them in order.

### 3. System Binary

If yt-dlp is installed as a standalone binary, updates may require manual intervention. Set the path in `.env`:

```env
YTDL_BIN=/path/to/yt-dlp
```

## Fragment Error Handling

Since we've enhanced yt-dlp parameters, fragment download failures are now handled more gracefully:

### New Parameters Added

```rust
--concurrent-fragments 3       // Reduced from 5 (less aggressive)
--fragment-retries 10          // Retry failed fragments 10 times
--socket-timeout 30            // 30-second socket timeout
--http-chunk-size 10485760     // 10MB chunks for granular retry
--sleep-requests 1             // 1ms delay between requests
```

### What This Means

- **Fewer concurrent fragments**: Reduces rate-limiting by YouTube
- **Fragment retries**: Automatically recovers from transient 403 errors
- **Socket timeout**: Prevents hanging connections
- **Chunk size**: Finer-grained control for resume/retry logic
- **Sleep requests**: Spacing reduces server strain

## Troubleshooting

### Problem: "403 Forbidden" Errors Persist

**Solution**: Update yt-dlp and check cookies

```bash
# Force update
cargo run -- update-ytdlp --force

# Check version
cargo run -- update-ytdlp --check

# Update cookies
export YOUTUBE_COOKIES_PATH=/path/to/cookies.txt
cargo run -- run
```

### Problem: "pip command not found"

**Solution**: Install Python and pip

macOS:
```bash
brew install python3
```

Ubuntu/Debian:
```bash
sudo apt-get install python3-pip
```

Then update:
```bash
pip3 install --upgrade yt-dlp
```

### Problem: Permission Denied on Update

**Solution**: Use pip with user install flag

```bash
pip install --user --upgrade yt-dlp
# or
pip3 install --user --upgrade yt-dlp
```

Or update system-wide with sudo:
```bash
sudo pip install --upgrade yt-dlp
```

### Problem: Update Timed Out

**Solution**: The update took too long (> 120 seconds)

Reasons:
- Slow internet connection
- PyPI server is slow
- System is overloaded

Try manually:
```bash
pip install --upgrade yt-dlp --timeout 300
```

## Scheduled Updates (Recommended)

### Option 1: Cron Job (Linux/macOS)

```bash
# Edit crontab
crontab -e

# Add this line to update daily at 2 AM
0 2 * * * cd /path/to/doradura && cargo run -- update-ytdlp >> /tmp/ytdlp-update.log 2>&1
```

### Option 2: Systemd Timer (Linux)

Create `/etc/systemd/system/ytdlp-update.timer`:
```ini
[Unit]
Description=Update yt-dlp daily

[Timer]
OnCalendar=daily
OnCalendar=*-*-* 02:00:00
Persistent=true

[Install]
WantedBy=timers.target
```

Create `/etc/systemd/system/ytdlp-update.service`:
```ini
[Unit]
Description=Update yt-dlp
After=network-online.target

[Service]
Type=oneshot
WorkingDirectory=/path/to/doradura
ExecStart=/usr/bin/cargo run -- update-ytdlp
User=your_user
```

Enable:
```bash
sudo systemctl daemon-reload
sudo systemctl enable ytdlp-update.timer
sudo systemctl start ytdlp-update.timer
```

### Option 3: Docker (If using Docker)

Add to startup script before running bot:
```bash
#!/bin/bash
cargo run -- update-ytdlp
cargo run -- run
```

## Version History

Check what changed in yt-dlp:
```bash
# View current version
yt-dlp --version

# View help for update flag
yt-dlp --help | grep -i update
```

## API Functions

If you need to programmatically update yt-dlp in your code:

```rust
use doradura::download::ytdlp;

// Check and update (only if needed)
ytdlp::check_and_update_ytdlp().await?;

// Force update
ytdlp::force_update_ytdlp().await?;

// Print current version
ytdlp::print_ytdlp_version().await?;
```

## Monitoring and Logging

All update attempts are logged to:
- **Console**: Real-time feedback
- **Log file**: Configured in `LOG_FILE_PATH` environment variable
- **Metrics**: Error tracking if update fails

Check logs:
```bash
tail -f logs/bot.log | grep -i "ytdlp\|update"
```

## Best Practices

1. **Update Regularly**: Run `update-ytdlp` weekly minimum
2. **Monitor Logs**: Watch for "403 Forbidden" increases (sign of outdated yt-dlp)
3. **Use System Packages**: Preferred over pip for production
4. **Enable Scheduled Updates**: Set up cron/systemd to auto-update
5. **Test After Updates**: Verify downloads work with test URLs
6. **Keep Cookies Fresh**: Combine yt-dlp updates with cookie refreshes

## FAQ

**Q: Will updating yt-dlp break my bot?**
A: No. Updates are backwards-compatible. The bot will continue working even if update fails.

**Q: How often does yt-dlp need updating?**
A: YouTube changes detection methods constantly. Update at least weekly, ideally daily.

**Q: Can I downgrade yt-dlp?**
A: Yes, if needed: `pip install yt-dlp==2024.01.01` (specify exact version)

**Q: Does the bot need to restart after update?**
A: Yes. Stop the bot, update, then restart.

**Q: How do I know if my yt-dlp is outdated?**
A: Look for "403 Forbidden" or "signature extraction failed" errors. These often indicate outdated yt-dlp.

## Related Documentation

- [FIX_YOUTUBE_ERRORS.md](FIX_YOUTUBE_ERRORS.md) - General YouTube error handling
- [YOUTUBE_COOKIES.md](YOUTUBE_COOKIES.md) - Cookie management
- [TROUBLESHOOTING.md](TROUBLESHOOTING.md) - General troubleshooting
