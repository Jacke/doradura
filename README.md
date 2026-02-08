<div align="center">

# Doradura

<img src="logo.webp" width="55%" height="350" alt="Doradura Logo">

### High-performance Telegram bot for downloading and converting media

[![Rust](https://img.shields.io/badge/rust-%23000000.svg?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Telegram](https://img.shields.io/badge/Telegram-2CA5E0?style=for-the-badge&logo=telegram&logoColor=white)](https://t.me/DoraDuraDoraDuraBot?start)

**Download media from multiple sources and convert between formats**

</div>

---

## Features

- **Multi-source downloads** — pluggable backend architecture via `DownloadSource` trait. Built-in support for 1000+ sites (YouTube, SoundCloud, TikTok, Instagram, VK, Twitch, Spotify, Bandcamp, and more) plus direct file URLs
- **Format conversion** — convert documents (DOCX to PDF), video (circles, GIF, compression), audio (effects, ringtones, cuts), and images (resize, format change) directly from sent files
- **Audio processing** — pitch, tempo, bass boost effects; ringtone creation; segment cutting with speed control
- **High performance** — built with Rust and async Tokio runtime for maximum throughput
- **Download pipeline** — source-agnostic pipeline with progress tracking, retry logic, and automatic cleanup
- **Queue system** — handle multiple concurrent downloads with per-user rate limiting and priority tiers
- **Subscription system** — Free / Premium / VIP tiers with Telegram Stars payments and auto-renewal
- **Multi-language** — Russian, English, French, German with per-user language selection
- **Database & history** — SQLite-backed download history, user stats, and export (TXT/CSV/JSON)

## Architecture

```
DownloadSource trait          SourceRegistry (URL routing)
  ├── YtDlpSource  ───────►  ┌──────────────────────┐
  │   (1000+ sites)           │  resolve(url) → src  │
  ├── HttpSource   ───────►  │  register(source)     │
  │   (direct files)          └──────────────────────┘
  └── (your backend)                    │
                                        ▼
                              Pipeline (download_phase / execute)
                                        │
                              ┌─────────┴─────────┐
                              │  audio.rs  video.rs │
                              └───────────────────┘

conversion/
  ├── video.rs    (circles, GIF, compression)
  ├── image.rs    (resize, format conversion)
  └── document.rs (DOCX → PDF via LibreOffice)
```

New download backends are added by implementing the `DownloadSource` trait and registering them in `SourceRegistry::default_registry()`.

## Quick Start

### Prerequisites

```bash
# Install FFmpeg (media processing)
brew install ffmpeg  # macOS
# or
sudo apt install ffmpeg  # Ubuntu/Debian

# Install yt-dlp (for platform downloads)
brew install yt-dlp  # macOS
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

# Run the bot (default mode)
./target/release/doradura run

# Or use cargo
cargo run -- run
```

> **Note:** The bot supports CLI commands. See [CLI_USAGE.md](CLI_USAGE.md) for all available commands including `run-staging`, `run-with-cookies`, and `refresh-metadata`.

### Environment Variables

Create a `.env` file in the project root:

```env
TELOXIDE_TOKEN=your_telegram_bot_token_here
ADMIN_USERNAME=your_telegram_username  # Admin user (without @)
YTDL_BIN=yt-dlp  # Optional: override default
BOT_API_URL=http://localhost:8081  # Optional: local Bot API server (files up to 2GB)
DOWNSUB_GRPC_ENDPOINT=http://localhost:50051  # Optional: Downsub gRPC for summarization/subtitles
```

### Local Bot API Server (Optional)

For sending files larger than 50 MB (up to 2 GB):

```bash
./scripts/start_local_bot_api.sh
```

See [LOCAL_BOT_API_SETUP.md](docs/LOCAL_BOT_API_SETUP.md) for details.

## Usage

Interact with the bot on Telegram:

- **Send a link** — downloads audio/video from supported platforms
- **Send a file** — offers conversion options
- `/start` — main menu
- `/settings` — download and conversion settings
- `/info <URL>` — show available formats for a URL
- `/downsub summary <URL>` — get a summary via Downsub
- `/downsub subtitles <URL>` — fetch subtitles via Downsub

## Testing

```bash
# Run all tests
cargo test

# Run with integration tests
cargo test -- --ignored

# Clippy
cargo clippy
```

## Dependencies

Key technologies:

- **[teloxide](https://github.com/teloxide/teloxide)** — Telegram bot framework
- **[tokio](https://tokio.rs/)** — async runtime
- **[rusqlite](https://github.com/rusqlite/rusqlite)** — SQLite database
- **[reqwest](https://github.com/seanmonstar/reqwest)** — HTTP client
- **[fluent-templates](https://github.com/XAMPPRocky/fluent-templates)** — i18n

## Documentation

- [QUICKSTART.md](docs/QUICKSTART.md)
- [BOT_FLOWS.md](docs/BOT_FLOWS.md)
- [LOCAL_BOT_API_SETUP.md](docs/LOCAL_BOT_API_SETUP.md)
- [YOUTUBE_COOKIES.md](docs/YOUTUBE_COOKIES.md)
- [SUBSCRIPTIONS.md](docs/SUBSCRIPTIONS.md)
- [TESTING.md](docs/TESTING.md)

## License

MIT License - see LICENSE file for details.

## Disclaimer

This bot is for personal use only. Please respect copyright laws and terms of service of the platforms you download from.

---

<div align="center">

Made with Rust by Dora

</div>
