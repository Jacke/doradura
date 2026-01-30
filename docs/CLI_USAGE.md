# CLI Usage

## Overview

The bot now supports CLI (Command Line Interface) with multiple subcommands for different operating modes.

## Installation and Build

```bash
cargo build --release
```

The executable will be located at `target/release/doradura`.

## Available Commands

### 1. `run` - Run bot in normal mode

Starts the bot in standard mode using environment variables from `.env`.

```bash
# Long polling mode (default)
./doradura run

# Webhook mode
./doradura run --webhook
```

**Without arguments (default):**
```bash
./doradura
# Equivalent to: ./doradura run
```

### 2. `run-staging` - Run bot in staging environment

Loads environment variables from `.env.staging` instead of `.env`.

```bash
# Long polling mode
./doradura run-staging

# Webhook mode
./doradura run-staging --webhook
```

**Usage:**
- Create a `.env.staging` file with test settings
- Run the bot with this environment file
- Useful for testing changes without affecting production

**Example `.env.staging`:**
```env
BOT_TOKEN=your_test_bot_token
DATABASE_PATH=database_staging.sqlite
ADMIN_USER_ID=123456789
# ... other variables
```

### 3. `run-with-cookies` - Run bot with cookies

Starts the bot with a specified path to the YouTube cookies file.

```bash
# With auto-detection of cookies path from environment variables
./doradura run-with-cookies

# With explicit path to cookies
./doradura run-with-cookies --cookies /path/to/youtube_cookies.txt

# Webhook mode
./doradura run-with-cookies --cookies /path/to/cookies.txt --webhook
```

**Usage:**
- Specifies the path to the cookies file to bypass YouTube restrictions
- Useful when updating cookies or testing new cookies
- If `--cookies` is not specified, the value from environment variables is used

### 4. `refresh-metadata` - Update metadata in download history

Scans the `download_history` table and updates missing metadata (file_size, duration, video_quality, audio_bitrate) for files that have already been successfully sent to Telegram.

```bash
# Update ALL entries with missing metadata
./doradura refresh-metadata

# Dry run - show what would be updated without making changes
./doradura refresh-metadata --dry-run

# Update only the first 10 entries
./doradura refresh-metadata --limit 10

# Verbose output (show each processed entry)
./doradura refresh-metadata --verbose

# Combination: dry run + verbose + limit
./doradura refresh-metadata --dry-run --verbose --limit 5
```

**Options:**
- `--limit <N>` - Process only the first N entries (useful for testing)
- `--dry-run` - Show what would be updated, but DON'T make changes to the database
- `--verbose` - Verbose output: show each processed entry

**How it works:**
1. Finds all entries in `download_history` with `file_id IS NOT NULL` and missing metadata
2. For each entry:
   - Downloads the file from Telegram using `file_id`
   - Extracts metadata using `ffprobe`:
     - `file_size` - file size in bytes
     - `duration` - duration in seconds
     - `video_quality` - video resolution (e.g., "1080p", "720p")
     - `audio_bitrate` - audio bitrate (e.g., "320k", "192k")
   - Updates the database entry
   - Deletes the temporary file
3. Outputs summary statistics

**Example output:**
```
Found 15 entries with missing metadata

[1/15] Processing: Rick Astley - Never Gonna Give You Up (format: mp3, file_id: AgAC...)
  Missing: file_size, duration, audio_bitrate
  Updated: Metadata { file_size: Some(3145728), duration: Some(213), audio_bitrate: Some("320k") }

[2/15] Processing: Example Video (format: mp4, file_id: BAADBAADAgI...)
  Missing: duration, video_quality
  Updated: Metadata { duration: Some(125), video_quality: Some("1080p") }

...

════════════════════════════════════════════════════════════
Metadata Refresh Summary:
   Total entries found: 15
   Successfully updated: 13
   Failed: 2
════════════════════════════════════════════════════════════
```

**When to use:**
- After migration from V9 to V10 (new fields added to download_history)
- When metadata was not saved due to an error
- To populate history of old downloads

**Requirements:**
- Installed `ffprobe` (part of FFmpeg)
- Access to Telegram Bot API
- `BOT_TOKEN` in environment variables

## Environment Variables

All commands use environment variables from `.env` (or `.env.staging` for `run-staging`):

```env
# Required
BOT_TOKEN=your_telegram_bot_token

# Optional
BOT_API_URL=http://localhost:8081              # Local Bot API (optional)
WEBHOOK_URL=https://yourdomain.com/webhook     # For webhook mode
YOUTUBE_COOKIES_PATH=/path/to/cookies.txt      # Path to YouTube cookies
DATABASE_PATH=database.sqlite                   # Path to database
ADMIN_USER_ID=123456789                        # Admin ID

# Metrics
METRICS_ENABLED=true
METRICS_PORT=9094

# Alerts
ALERTS_ENABLED=true

# Mini App
WEBAPP_PORT=8080

# ... and other variables from config.rs
```

## Migration from Scripts

### Before:

**run_staging.sh:**
```bash
#!/bin/bash
export $(cat .env.staging | xargs)
cargo run
```

**run_with_cookies.sh:**
```bash
#!/bin/bash
export YOUTUBE_COOKIES_PATH=/path/to/cookies.txt
cargo run
```

### After:

```bash
# Instead of run_staging.sh
./doradura run-staging

# Instead of run_with_cookies.sh
./doradura run-with-cookies --cookies /path/to/cookies.txt
```

**Benefits:**
- No separate scripts needed
- Single entry point
- Built-in documentation (`--help`)
- Type-safe arguments
- Command auto-completion (with shell completion)

## Usage Examples

### Development

```bash
# Run in normal mode
cargo run -- run

# Run in staging
cargo run -- run-staging

# Update metadata (dry run)
cargo run -- refresh-metadata --dry-run --verbose --limit 5
```

### Production

```bash
# Build release version
cargo build --release

# Run the bot
./target/release/doradura run

# Systemd service (example)
[Service]
ExecStart=/path/to/doradura run
Restart=always
```

### Updating Metadata

```bash
# 1. First dry run to see what will be updated
./doradura refresh-metadata --dry-run --verbose

# 2. Update first 10 for testing
./doradura refresh-metadata --limit 10 --verbose

# 3. If everything is ok, update all
./doradura refresh-metadata
```

## Docker

If using Docker, update `CMD` in `Dockerfile`:

```dockerfile
# Before
CMD ["./doradura"]

# After (explicitly specify command)
CMD ["./doradura", "run"]
```

Or use arguments when running:

```bash
# Normal mode
docker run mybot run

# Staging mode
docker run mybot run-staging

# Refresh metadata
docker run mybot refresh-metadata --limit 100
```

## Railway Deployment

Update the start command in Railway settings:

```bash
# Instead of: ./doradura
# Use: ./doradura run

# Or with webhook:
./doradura run --webhook
```

## Shell Completion (Optional)

Clap supports generating auto-completion for various shells:

```bash
# For bash
doradura --generate-completion bash > /etc/bash_completion.d/doradura

# For zsh
doradura --generate-completion zsh > /usr/local/share/zsh/site-functions/_doradura

# For fish
doradura --generate-completion fish > ~/.config/fish/completions/doradura.fish
```

(Requires adding `clap_complete` feature and generation code)

## Troubleshooting

### "BOT_TOKEN environment variable not set"

Make sure the `.env` file exists and contains `BOT_TOKEN`:

```bash
# Check
cat .env | grep BOT_TOKEN

# Or run with explicit specification
BOT_TOKEN=your_token ./doradura run
```

### "Failed to create database pool"

Check access permissions to the database file:

```bash
ls -la database.sqlite

# If needed
chmod 644 database.sqlite
```

### Errors during refresh-metadata

**"Failed to run ffprobe":**
```bash
# Install ffmpeg
# macOS:
brew install ffmpeg

# Ubuntu/Debian:
sudo apt-get install ffmpeg

# Check
ffprobe -version
```

**"Failed to download file from Telegram":**
- Check that `BOT_TOKEN` is correct
- Check internet connection
- Check that the file has not been deleted from Telegram

## Roadmap

Planned additional commands:

- `doradura backup` - Create database backup
- `doradura stats` - Show usage statistics
- `doradura migrate` - Run database migrations
- `doradura clean` - Clean temporary files
- `doradura export` - Export data to CSV/JSON

## See also

- [README.md](README.md) - Main documentation
- [ERROR_METRICS_COMPREHENSIVE.md](ERROR_METRICS_COMPREHENSIVE.md) - Error metrics
- [ANALYTICS_SYSTEM.md](ANALYTICS_SYSTEM.md) - Analytics system
