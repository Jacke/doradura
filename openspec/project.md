# Project Context

## Purpose
Doradura is a high-performance Telegram bot built in Rust for downloading music and videos from YouTube and SoundCloud. The bot provides:
- High-quality audio downloads (MP3, 320kbps) with embedded metadata and thumbnails
- Video downloads in best available quality
- Subtitle extraction support
- User history tracking and statistics
- Subscription-based rate limiting (free/premium/vip plans)
- Queue-based download processing
- Automatic retry logic for reliable file delivery

The bot is designed for personal use with Russian language interface ("Я Дора" - "I'm Dora").

## Tech Stack
- **Rust** (edition 2021) - Core language
- **teloxide** (v0.17) - Telegram bot framework
- **tokio** (v1.8) - Async runtime
- **rusqlite** (v0.31) + **r2d2** - SQLite database with connection pooling
- **reqwest** (v0.12) - HTTP client for API requests
- **anyhow** (v1.0) - Error handling
- **chrono** (v0.4) - Date/time handling
- **simplelog** - Logging (console + file)
- **regex** - URL pattern matching
- **yt-dlp** / **youtube-dl** - External command-line tools for media downloads
- **ffmpeg** - External tool for audio/video processing

## Project Conventions

### Code Style
- Follow Rust standard formatting (`rustfmt`)
- Use `snake_case` for functions, variables, and modules
- Use `PascalCase` for types, structs, and enums
- Use `SCREAMING_SNAKE_CASE` for constants
- Prefer `///` doc comments for public API documentation
- Include `# Errors` section in doc comments when applicable
- Use meaningful variable names (prefer clarity over brevity)
- Prefer `anyhow::Result` for error handling in application code
- Use `Arc` for shared state across async tasks
- Module organization: one module per file under `src/`

### Architecture Patterns
- **Modular design**: Separate modules for different concerns (commands, downloader, queue, db, etc.)
- **Async/await**: All I/O operations use async/await with tokio runtime
- **Shared state**: Use `Arc` for thread-safe shared references (database pool, rate limiter, queue)
- **Dependency injection**: Pass dependencies (bot, db_pool, rate_limiter) as function parameters
- **Separation of concerns**:
  - `commands.rs` - Message handling and URL parsing
  - `downloader.rs` - Download logic with retry mechanisms
  - `queue.rs` - Thread-safe download queue processing
  - `rate_limiter.rs` - Per-user rate limiting
  - `db.rs` - Database operations
  - `progress.rs` - Download progress tracking
- **Error handling**: Use `anyhow::Result` for application errors, `thiserror` for structured error types
- **Configuration**: Environment variables via `.env` file (dotenvy)
- **Logging**: Structured logging with `simplelog` (console + file `app.log`)

### Testing Strategy
- Use `cargo test` for running tests
- Tests are located in `tests/` directory for integration tests
- Module-level unit tests use `#[cfg(test)]` blocks
- Use `--ignored` flag for integration tests that require external dependencies
- Test specific modules: `cargo test --test [test_name]`
- Mock external dependencies where possible (e.g., `wiremock` for HTTP mocking)

### Git Workflow
- Standard git workflow (details not explicitly documented)
- Commit messages should be clear and descriptive
- Use feature branches for new functionality

## Domain Context
- **Bot name**: "Дора" (Dora) - Russian-language Telegram bot
- **User interface**: Primarily Russian language ("Я Дора, чай закончился...")
- **Subscription plans**: `free`, `premium`, `vip` (stored in database)
- **Download formats**: `mp3` (audio), `mp4` (video), `srt` (subtitles), `txt` (subtitles text)
- **Video quality options**: `best`, `1080p`, `720p`, `480p`, `360p`
- **Audio bitrate options**: `128k`, `192k`, `256k`, `320k`
- **Rate limiting**: Per-user cooldown periods (configurable per plan)
- **Download location**: `~/downloads/` (configurable)
- **Supported platforms**: YouTube, SoundCloud
- **Database**: SQLite (`database.sqlite`) with migrations (`migrations/`)
- **Admin functionality**: User management, backups, subscription management

## Important Constraints
- **File size limits**: 
  - Standard Telegram API: 50 MB max
  - Local Bot API server: up to 2 GB (optional, configured via `BOT_API_URL`)
- **Rate limiting**: Per-user cooldown periods prevent abuse (default 30 seconds, configurable per plan)
- **External dependencies**: Requires `yt-dlp`/`youtube-dl` and `ffmpeg` installed on system
- **Database**: SQLite (single-file database, backups required for production)
- **Environment**: Requires `TELOXIDE_TOKEN` environment variable
- **Personal use**: Bot is designed for personal use; respect copyright laws and platform terms of service
- **Retry logic**: Default 3 attempts with 10-second delays for failed downloads

## External Dependencies
- **Telegram Bot API**: Via teloxide library
  - Standard API: `https://api.telegram.org` (default)
  - Local Bot API server: Configured via `BOT_API_URL` env var (optional)
- **yt-dlp / youtube-dl**: Command-line tool for downloading media
  - Auto-updated on startup via `ytdlp::check_and_update_ytdlp()`
  - Configurable via `YTDL_BIN` environment variable
- **ffmpeg**: Required for audio/video processing and conversion
  - Used for WAV to OGG Opus conversion (for waveform display)
  - Used for audio encoding/extraction
- **System paths**: 
  - Downloads stored in `~/downloads/` (shell expansion supported)
  - Database at `database.sqlite` (project root)
  - Logs written to `app.log` (project root)
