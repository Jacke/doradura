<div align="center">

# 🎵 Doradura

<img src="logo.webp" width="200" height="200" alt="Doradura Logo">

### High-performance Telegram bot for downloading music and videos

[![Rust](https://img.shields.io/badge/rust-%23000000.svg?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Telegram](https://img.shields.io/badge/Telegram-2CA5E0?style=for-the-badge&logo=telegram&logoColor=white)](https://telegram.org/)

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
```

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

Files are downloaded to: `~/downloads/`

Change in `src/downloader.rs`:

```rust
let full_path = format!("~/downloads/{}", safe_filename);
```

### Retry Logic

Default: **3 attempts** with **10 second** delays

Configure in `src/downloader.rs`:

```rust
let max_attempts = 3;
// ...
tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
```

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
