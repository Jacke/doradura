# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Doradura is a Telegram bot written in Rust that downloads audio and video content from YouTube and SoundCloud. The bot is designed with a queue-based architecture, rate limiting, and includes C code integration for specific functionality. The bot responds in Russian and has a friendly personality ("Dora").

## Build and Development Commands

### Building
```bash
cargo build
cargo build --release
```

**IMPORTANT FOR CLAUDE CODE:**
- **DO NOT automatically start or restart the bot after building**
- Only run `cargo build` or `cargo build --release`
- The bot is managed manually by the user
- If you need to restart the bot, ask the user first

### Running (Manual - User Only)
```bash
# Set required environment variable first
export TELOXIDE_TOKEN="your_telegram_bot_token"

# Optional: override default downloader (defaults to youtube-dl)
export YTDL_BIN="yt-dlp"  # or "youtube-dl"

cargo run
```

### Testing
```bash
# Run all tests
cargo test

# Run specific test module
cargo test --test download_video

# Run tests including ignored ones (requires yt-dlp/youtube-dl and ffmpeg installed)
cargo test -- --ignored
```

### Environment Setup
The application requires a `.env` file with:
```
TELOXIDE_TOKEN=your_telegram_bot_token
```

Optional:
```
YTDL_BIN=yt-dlp  # Override default youtube-dl binary
```

### External Dependencies
The bot requires these system tools to be installed:
- `youtube-dl` or `yt-dlp` (video/audio downloader)
- `ffmpeg` and `ffprobe` (audio/video processing)

## Architecture

### Core Components

1. **Main Entry Point** ([src/main.rs](src/main.rs))
   - Initializes the bot with teloxide framework
   - Sets up logging to both console (Error level) and `app.log` file
   - Runs database migrations from `migrations/` on startup
   - Creates shared state: `RateLimiter` and `DownloadQueue`
   - Spawns async `process_queue` task that continuously processes downloads
   - Sets up command handlers (`/start`, `/help`, `/settings`, `/tasks`) and message handlers
   - Implements exponential backoff retry logic for dispatcher errors
   - Calls C functions `foo()` and `bar()` from linked C code

2. **Download Queue System** ([src/queue.rs](src/queue.rs))
   - Thread-safe `DownloadQueue` using `Mutex<VecDeque<DownloadTask>>`
   - Each `DownloadTask` contains: URL, chat_id, is_video flag, and creation timestamp
   - Supports adding tasks, retrieving tasks (FIFO), filtering by chat_id, and removing old tasks
   - Used to decouple request handling from actual download processing

3. **Rate Limiting** ([src/rate_limiter.rs](src/rate_limiter.rs))
   - Per-chat rate limiting using `HashMap<ChatId, Instant>`
   - Default limit: 30 seconds between downloads per user
   - Async-safe with tokio Mutex
   - Tracks when each chat_id can make their next request

4. **Download Processing** ([src/downloader.rs](src/downloader.rs))
   - `download_and_send_audio`: Downloads audio as MP3 (320kbps), extracts metadata, embeds thumbnail
   - `download_and_send_video`: Downloads best quality video format
   - Both spawn async tasks that:
     - Fetch metadata (title/artist) from URL
     - Generate safe filename and download to `~/downloads/`
     - Use `youtube-dl` or `yt-dlp` (with fallback logic)
     - Probe duration using `ffprobe`
     - Send file via Telegram with retry logic (3 attempts, 10s delay)
     - Auto-cleanup files after 600 seconds
   - Error handling with custom `CommandError` enum

5. **Command Handler** ([src/commands.rs](src/commands.rs))
   - Parses incoming messages for URLs using regex
   - Removes `&list` parameter from YouTube URLs to avoid playlists
   - Detects video downloads by checking for "video " prefix in message
   - Checks rate limits before queuing downloads
   - Provides Russian-language user feedback

6. **Metadata Fetching** ([src/fetch.rs](src/fetch.rs))
   - Fetches HTML from URL and parses with `select` crate
   - Extracts `<title>` and `og:artist` meta tags
   - Custom error type `FetchError` for HTTP errors

7. **Database Layer** ([src/db.rs](src/db.rs))
   - SQLite database (`database.sqlite`) for persistence
   - Manages users (telegram_id, username, plan)
   - Logs all requests to `request_history` table
   - Schema defined in `migrations/`

8. **C Code Integration** ([build.rs](build.rs), [c_code/](c_code/))
   - Build script compiles `foo.c` and `bar.c` into static library
   - Functions `foo()` and `bar()` called via FFI in main.rs
   - Uses `cc` crate for compilation

### Key Flow

1. User sends URL to bot
2. Message handler in [src/commands.rs](src/commands.rs) validates URL and checks rate limit
3. Task added to `DownloadQueue`
4. Background `process_queue` loop picks up task
5. Downloader spawns async task to:
   - Fetch metadata
   - Download file using youtube-dl/yt-dlp
   - Send to user via Telegram
   - Clean up after delay
6. Database logs the request

### File Structure

```
src/
├── main.rs          - Entry point, bot setup, dispatcher
├── commands.rs      - Message/command handling logic
├── downloader.rs    - Audio/video download and send
├── queue.rs         - Download queue implementation
├── rate_limiter.rs  - Per-user rate limiting
├── db.rs            - SQLite database operations
├── fetch.rs         - URL metadata scraping
└── utils.rs         - Utility functions (filename escaping, etc.)
tests/
└── download_video.rs - Integration test for video downloads
c_code/
├── foo.c           - C code linked via FFI
└── bar.c           - C code linked via FFI
build.rs            - Build script for C code compilation
migrations/         - Database migrations
```

### Testing Notes

- Unit tests are embedded in source files using `#[cfg(test)]`
- Main unit tests in [src/main.rs](src/main.rs) focus on queue operations
- Integration test [tests/download_video.rs](tests/download_video.rs) is marked `#[ignore]` and requires network access and external tools
- Download tests in [src/downloader.rs](src/downloader.rs) check tool availability before running

### Important Patterns

- Heavy use of `Arc<T>` for sharing state between async tasks
- Error handling with `thiserror` for custom error types and `anyhow` for error context
- Graceful degradation: fallback from youtube-dl to yt-dlp if primary binary not found
- Retry logic for both Telegram API calls and dispatcher errors
- Auto-cleanup of downloaded files after 10 minutes to save disk space
