<div align="center">

# 🎵 Doradura

<img src="logo.webp" width="55%" height="350" alt="Doradura Logo">

### High-performance Telegram bot for downloading music and videos

[![Rust](https://img.shields.io/badge/rust-%23000000.svg?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Telegram](https://img.shields.io/badge/Telegram-2CA5E0?style=for-the-badge&logo=telegram&logoColor=white)](https://t.me/DoraDuraDoraDuraBot?start)

**Download audio & video from YouTube and SoundCloud with ease**

</div>

---

## ✨ Features

- 🎵 **Audio Downloads** - High-quality MP3 downloads (320kbps) with embedded metadata and thumbnails
- 🎥 **Video Downloads** - Get videos in the best available quality
- ⚡ **Fast & Efficient** - Built with Rust for maximum performance
- 🔄 **Retry Logic** - Automatic retries for reliable file delivery
- 🛡️ **Rate Limiting** - Prevent abuse with intelligent rate limiting (30s cooldown per user)
- 📊 **Queue System** - Handle multiple downloads seamlessly
- 🗄️ **Database Logging** - Track all user requests and history
- 🔧 **Multi-Platform** - Support for both `youtube-dl` and `yt-dlp`
- 💾 **Auto-Cleanup** - Temporary files are automatically removed after sending

## 🚀 Quick Start

### Prerequisites

Install the required system tools:

```bash
# Install FFmpeg (audio/video processing)
brew install ffmpeg  # macOS
# or
sudo apt install ffmpeg  # Ubuntu/Debian

# Install youtube-dl or yt-dlp
brew install yt-dlp  # macOS (recommended)
# or
pip install yt-dlp  # Python alternative
```

### Installation

1. Clone the repository:

```bash
git clone https://github.com/yourusername/doradura.git
cd doradura
```

2. Create a `.env` file:

```bash
cp .env.example .env
# Edit .env and add your Telegram bot token
```

3. Build and run:

```bash
cargo build --release
cargo run
```

### Environment Variables

Create a `.env` file in the project root:

```env
TELOXIDE_TOKEN=your_telegram_bot_token_here
YTDL_BIN=yt-dlp  # Optional: override default youtube-dl
BOT_API_URL=http://localhost:8081  # Optional: use local Bot API server (allows files up to 2GB)

# YouTube Cookies (required for YouTube downloads)
# Option 1: Automatic extraction from browser (Linux/Windows recommended)
YTDL_COOKIES_BROWSER=chrome  # chrome, firefox, safari, brave, chromium, edge, opera, vivaldi

# Option 2: Export cookies to file (macOS recommended)
YTDL_COOKIES_FILE=youtube_cookies.txt
```

**📋 Quick Setup for Cookies:**

**Linux/Windows (Automatic):**
```bash
# 1. Install dependencies
pip3 install keyring pycryptodomex

# 2. Login to YouTube in your browser
# 3. Set environment variable
export YTDL_COOKIES_BROWSER=chrome

# 4. Restart bot
```

**macOS (File-based, recommended):**
```bash
# 1. Export cookies using browser extension (see MACOS_COOKIES_FIX.md)
# 2. Set environment variable
export YTDL_COOKIES_FILE=youtube_cookies.txt

# 3. Restart bot
```

See [docs/YOUTUBE_COOKIES.md](docs/YOUTUBE_COOKIES.md) for detailed instructions.

### 🚀 Local Bot API Server (Optional)

For sending files larger than 50 MB (up to 2 GB), you can use a local Bot API server:

1. **Quick start with Docker:**
   ```bash
   ./start_local_bot_api.sh
   ```
   
   See [LOCAL_BOT_API_SETUP.md](docs/LOCAL_BOT_API_SETUP.md) for detailed instructions.

2. **Benefits:**
   - Upload files up to **2 GB** (instead of 50 MB)
   - Lower latency
   - More flexibility with webhooks

## 📖 Usage

Once running, interact with the bot on Telegram:

- **Send a YouTube/SoundCloud link** - Downloads audio by default
- **Send "video" + link** - Downloads video instead
- `/start` - Shows welcome message
- `/help` - Display bot commands
- `/settings` - View your settings
- `/tasks` - Check active downloads

### Examples

```
https://youtube.com/watch?v=...
https://soundcloud.com/...
video https://youtube.com/watch?v=...
```

## 🏗️ Architecture

### Core Components

- **`src/main.rs`** - Bot initialization, dispatcher setup, queue processing
- **`src/downloader.rs`** - Audio/video download logic with retry mechanisms
- **`src/queue.rs`** - Thread-safe download queue system
- **`src/rate_limiter.rs`** - Per-user rate limiting
- **`src/commands.rs`** - Message and URL parsing handlers
- **`src/db.rs`** - SQLite database for logging
- **`src/fetch.rs`** - Metadata fetching from URLs

## 🧪 Testing

```bash
# Run all tests
cargo test

# Run tests including integration tests
cargo test -- --ignored

# Test specific module
cargo test --test download_video
```

## 🛠️ Development

### Build

```bash
# Development build
cargo build

# Release build (optimized)
cargo build --release
```

### Run

```bash
# Run with default settings
cargo run

# Run with custom logger
RUST_LOG=info cargo run
```

## 📦 Dependencies

Key technologies used:

- **[teloxide](https://github.com/teloxide/teloxide)** - Modern Telegram bot framework
- **[tokio](https://tokio.rs/)** - Async runtime
- **[rusqlite](https://github.com/rusqlite/rusqlite)** - Database integration
- **[reqwest](https://github.com/seanmonstar/reqwest)** - HTTP client
- **[anyhow](https://github.com/dtolnay/anyhow)** - Error handling
- **[chrono](https://github.com/chronotope/chrono)** - Date and time handling

## 🔧 Configuration

### Rate Limiting

Default rate limit: **30 seconds** between downloads per user

To modify, edit `src/main.rs`:

```rust
let rate_limiter = Arc::new(RateLimiter::new(Duration::from_secs(30)));
```

### Download Location

Files are downloaded to a configurable folder with platform-specific defaults:

- **macOS**: `~/downloads/dora-files/`
- **Other platforms**: `~/downloads/`

To customize the download folder, set the `DOWNLOAD_FOLDER` environment variable in your `.env` file:

```env
DOWNLOAD_FOLDER=~/downloads/my-custom-folder
```

The path supports tilde (`~`) expansion for the home directory.

### Retry Logic

Default: **3 attempts** with **10 second** delays

Configure in `src/downloader.rs`:

```rust
let max_attempts = 3;
// ...
tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
```

## 📚 Documentation

- [AGENTS.md](docs/AGENTS.md)
- [BOT_FLOWS.md](docs/BOT_FLOWS.md)
- [CACHE_ISSUE.md](docs/CACHE_ISSUE.md)
- [CLAUDE.md](docs/CLAUDE.md)
- [CODE_QUALITY_ANALYSIS.md](docs/CODE_QUALITY_ANALYSIS.md)
- [COOKIE_FIX_SUMMARY.md](docs/COOKIE_FIX_SUMMARY.md)
- [FILENAME_FIX.md](docs/FILENAME_FIX.md)
- [FIX_UNKNOWN_TRACK.md](docs/FIX_UNKNOWN_TRACK.md)
- [FIX_YOUTUBE_ERRORS.md](docs/FIX_YOUTUBE_ERRORS.md)
- [IDEAS.md](docs/IDEAS.md)
- [IMPROVEMENTS.md](docs/IMPROVEMENTS.md)
- [LOCAL_BOT_API_SETUP.md](docs/LOCAL_BOT_API_SETUP.md)
- [MACOS_COOKIES_FIX.md](docs/MACOS_COOKIES_FIX.md)
- [OPTIMIZATION_OPPORTUNITIES.md](docs/OPTIMIZATION_OPPORTUNITIES.md)
- [OPTIMIZATION_REALISTIC_ANALYSIS.md](docs/OPTIMIZATION_REALISTIC_ANALYSIS.md)
- [PROGRESS_BAR_FIX.md](docs/PROGRESS_BAR_FIX.md)
- [QUICKSTART.md](docs/QUICKSTART.md)
- [QUICK_FIX.md](docs/QUICK_FIX.md)
- [REMAINING_TASKS.md](docs/REMAINING_TASKS.md)
- [RUN_TESTS.md](docs/RUN_TESTS.md)
- [SESSION_SUMMARY.md](docs/SESSION_SUMMARY.md)
- [SUBSCRIPTIONS.md](docs/SUBSCRIPTIONS.md)
- [TESTING.md](docs/TESTING.md)
- [TEST_SUMMARY.md](docs/TEST_SUMMARY.md)
- [VIDEO_BLACK_SCREEN_FIX.md](docs/VIDEO_BLACK_SCREEN_FIX.md)
- [VIDEO_BLACK_SCREEN_FIX_V2.md](docs/VIDEO_BLACK_SCREEN_FIX_V2.md)
- [YOUTUBE_COOKIES.md](docs/YOUTUBE_COOKIES.md)
- [YOUTUBE_PO_TOKEN_FIX.md](docs/YOUTUBE_PO_TOKEN_FIX.md)

## 📝 License

MIT License - see LICENSE file for details

## 🤝 Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## ⚠️ Disclaimer

This bot is for personal use only. Please respect copyright laws and terms of service of the platforms you download from.

## 💡 Credits

Built with ❤️ using Rust and the amazing teloxide library.

---

<div align="center">

Made with 🔥 by Dora

</div>
