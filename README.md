<div align="center">

<img src="logo.webp" width="52%" alt="Doradura Logo">

# doradura

**Two ways to download the internet. One codebase. Pure Rust.**

[![Rust](https://img.shields.io/badge/Rust-1.83+-000000?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Telegram Bot](https://img.shields.io/badge/Telegram_Bot-v0.13-2CA5E0?style=for-the-badge&logo=telegram&logoColor=white)](https://t.me/DoraDuraDoraDuraBot?start)
[![TUI](https://img.shields.io/badge/TUI-dora_v0.6.6-A6E3A1?style=for-the-badge&logo=gnometerminal&logoColor=black)](https://github.com/Jacke/doradura)
[![License: MIT](https://img.shields.io/badge/License-MIT-CBA6F7?style=for-the-badge)](LICENSE)

</div>

---

```
╔══════════════════════════════════════════════════════════════════════════╗
║  doradura  =  dora TUI  +  doradura Telegram Bot                        ║
║  1000+ platforms  ·  async Rust  ·  Catppuccin Mocha  ·  production     ║
╚══════════════════════════════════════════════════════════════════════════╝
```

**doradura** is a media-download ecosystem built entirely in Rust. It ships two production products that share one high-performance core:

- **dora TUI** — a Catppuccin-themed terminal client for your desktop (macOS / Linux)
- **doradura Telegram Bot** — an enterprise-grade bot with subscriptions, audio FX, 9+ output formats, and 1000+ supported platforms

<img src="dora-tui.png" width="100%" alt="dora TUI — terminal media downloader">

---

## Products at a glance

| | dora TUI | doradura Bot |
|---|---|---|
| **Interface** | Terminal (ratatui, full mouse) | Telegram |
| **Version** | 0.6.6 | 0.13.0 |
| **Use case** | Personal power-user desktop client | Shared / team / public bot |
| **Platforms** | macOS, Linux | Any (deployed on Railway) |
| **Formats** | MP3, MP4 | MP3, MP4, GIF, WAV, FLAC, OGG, SRT, M4A, M4R |
| **Lyrics** | LRCLIB + Genius search | — |
| **Subscriptions** | — | Free / Premium / VIP |
| **Audio FX** | — | Pitch · Tempo · Bass · Lofi · Wide · Morph |

---

## dora — Beautiful TUI Media Downloader

```
 ╭──────────────────────────────────────────────────────────────────╮
 │   ██████╗  ██████╗ ██████╗  █████╗                               │
 │   ██╔══██╗██╔═══██╗██╔══██╗██╔══██╗                              │
 │   ██║  ██║██║   ██║██████╔╝███████║                              │
 │   ██║  ██║██║   ██║██╔══██╗██╔══██║                              │
 │   ██████╔╝╚██████╔╝██║  ██║██║  ██║                              │
 │   ╚═════╝  ╚═════╝ ╚═╝  ╚═╝╚═╝  ╚═╝                             │
 │                                                                  │
 │  [1] ⬇  Downloads   [2] 🎵 Lyrics   [3] ⚙  Settings             │
 ╰──────────────────────────────────────────────────────────────────╯
```

A pixel-perfect TUI built with **ratatui** and the **Catppuccin Mocha** colour palette — 7 cycleable logo themes, full mouse support, 60 fps rendering.

### Interface

| Tab | What lives here |
|-----|----------------|
| **[1] Downloads** | URL input · live download queue · full scrollable history with pop-up details |
| **[2] Lyrics** | Artist + title search via LRCLIB/Genius with full scrollable lyrics |
| **[3] Settings** | yt-dlp path, output folder, quality, bitrate, rate-limit, cookies — persisted to `~/.config/dora/settings.json` |

### Download Flow

1. Paste a URL → **Enter** — a rich preview popup appears with thumbnail ASCII art, title, channel and available qualities
2. Toggle `Tab` to switch MP3 ↔ MP4; use `←/→` to pick resolution
3. Press **Enter** — download starts in the queue with real-time speed + ETA
4. On completion the entry moves to History; press **r** or click the ASCII art panel to **Reveal in Finder / Files**

### Quality & Format Options

| Audio | Video |
|-------|-------|
| MP3 — 128k / 192k / 256k / 320k | MP4 — 360p / 480p / 720p / 1080p / best |

### Settings

```
yt-dlp ─────────────────────────────────────────────────────────────
  Binary path        yt-dlp
  Output folder      ~/Downloads
  Audio bitrate   ←  320k  →
  Video quality   ←  1080p →
  Rate limit      ←  off   →
  Cookies file       (none)

Instagram ──────────────────────────────────────────────────────────
  Cookies file       (none)
  GraphQL Doc ID     (none)

Conversion ─────────────────────────────────────────────────────────
  Default format  ←  MP3   →
  MP3 bitrate     ←  320k  →
```

### Keyboard Reference

```
Global                Downloads               History popup
──────────────────    ──────────────────────  ──────────────────────
1 / 2 / 3  Tabs       Enter  Start / preview  r / Enter  Reveal
?          Help        r      Reveal latest    b          Open in browser
Esc        Close       d      Remove slot      d          Delete entry
Ctrl+C     Quit        c      Cookies file     Esc        Close
                       ↑ / ↓  Scroll history

Preview popup         Lyrics                  Settings
──────────────────    ──────────────────────  ──────────────────────
Tab    MP3 ↔ MP4      Enter  Search           ↑ / ↓  Navigate
← / →  Quality        ↑ / ↓  Scroll           ← / →  Cycle value
Enter  Download        Esc    Clear            Enter  Edit text field
Esc    Cancel                                  s      Save · r  Reset
```

### yt-dlp integration

- **Startup check** — missing binary shows an install-or-quit dialog
- **Auto-update** — `yt-dlp -U` runs in the background on launch; a progress strip fades in and out when done
- **Cookies dialog** — drag-and-drop or browse cookies.txt for authenticated downloads

### Logo themes

Click the logo to cycle through 7 themes — each click fires a burst animation:
`Catppuccin` · `Fire` · `Ice` · `Matrix` · `Sunset` · `Neon` · `Gold`

### Install dora

**macOS — Homebrew:**
```bash
brew tap Jacke/dora
brew install dora
```

**Ubuntu / Debian — apt:**
```bash
curl -fsSL https://iamjacke.com/doradura/KEY.gpg | sudo gpg --dearmor -o /etc/apt/keyrings/dora.gpg
echo "deb [signed-by=/etc/apt/keyrings/dora.gpg] https://iamjacke.com/doradura stable main" | sudo tee /etc/apt/sources.list.d/dora.list
sudo apt update && sudo apt install dora
```

**Arch Linux — AUR:**
```bash
yay -S dora-bin
# or: paru -S dora-bin
```

**Linux / macOS — shell installer:**
```bash
curl -sSfL https://github.com/Jacke/doradura/releases/latest/download/dora-installer.sh | sh
```

**Build from source:**
```bash
cargo install --path crates/doratui
```

**Runtime dependencies:** [`yt-dlp`](https://github.com/yt-dlp/yt-dlp) · [`ffmpeg`](https://ffmpeg.org)

### Run

```bash
dora

# Demo mode (pre-populated with sample data)
dora --demo
```

---

## doradura — Enterprise Telegram Bot

A production-grade Telegram bot deployed on Railway, serving **1000+ platforms**, real-time audio processing, subscription tiers, and a Prometheus-monitored download pipeline.

### Supported Platforms

YouTube · SoundCloud · TikTok · Instagram · VK · Twitch · Spotify · Bandcamp · Twitter/X · Dailymotion · Vimeo · Reddit · Facebook · and 1000+ more via yt-dlp

### Commands

| Command | Description |
|---------|-------------|
| `/start` | Main menu |
| `/download <url>` | Download directly |
| `/info <url>` | Show metadata + available formats |
| `/history` | Your download history |
| `/stats` | Your usage statistics |
| `/plan` | Subscription info |
| `/settings` | Preferences (quality, bitrate, language, format) |
| `/feedback` | Send feedback to the admin |

### Download Formats

| Format | Quality |
|--------|---------|
| **MP3** | 128k · 192k · 256k · 320k |
| **MP4** | 360p · 480p · 720p · 1080p · best |
| **Video Note** | Auto-split circles (up to 6 × 60s) |
| **GIF** | Converted from video |
| **Audio** | WAV · FLAC · OGG · M4A · Opus · AAC |
| **Subtitles** | SRT · TXT (via Downsub gRPC) |
| **Ringtones** | iPhone `.m4r` (≤30s) · Android `.mp3` (≤40s) |

### Audio Effects Engine

Available on Premium+, applied on-the-fly with FFmpeg:

```
Pitch        −12 to +12 semitones
Tempo        0.5× to 2.0× (pitch-preserved)
Bass Boost   −12 to +12 dB

Morph profiles:
  Soft        Vocal-optimised low-cut
  Aggressive  Compressed · crushed · echoed
  Lofi        22 kHz downsampling + vinyl grain
  Wide        Stereo enhancement · extra-stereo
```

### Multi-language

**English** · **Русский** · **Français** · **Deutsch** — per-user language preference stored in the database, powered by Fluent localisation.

### Infrastructure

```
Telegram ──► teloxide long-poll / webhook
               │
               ▼
         Download Pipeline (crates/core)
          ├─ SourceRegistry (URL routing)
          │    ├─ YtDlpSource   (1000+ sites · Deno JS runtime · PO token fallback)
          │    └─ HttpSource    (direct URLs · chunked · resumable)
          ├─ Audio Pipeline    (FFmpeg · effects · cutting · ringtones)
          ├─ Video Pipeline    (circles · GIF · burn-in · splitting)
          └─ Subtitle Cache    (disk-permanent · Downsub gRPC)
               │
               ▼
         SQLite (r2d2 pool)
          ├─ Users + subscriptions + language prefs
          ├─ Download history + task queue
          ├─ Audio effect sessions  (24h TTL)
          ├─ Cut sessions           (10 min TTL)
          └─ Bot asset cache        (Telegram file_id)
               │
               ▼
         Prometheus metrics  /  Admin alert system
```

### Deploy on Railway

```bash
# 1. Fork and connect repo to Railway
# 2. Set environment variables (see below)
# 3. Push → automatic Docker build + deploy

railway up
```

#### Required environment variables

```env
TELOXIDE_TOKEN=your_telegram_bot_token
TELEGRAM_API_ID=your_api_id
TELEGRAM_API_HASH=your_api_hash
ADMIN_USERNAME=your_username           # without @
DATABASE_URL=/data/database.sqlite
WARP_PROXY=socks5://...               # CRITICAL for YouTube on Railway
```

#### Optional

```env
BOT_API_URL=http://localhost:8081     # local Bot API — files up to 2 GB
DOWNSUB_GRPC_ENDPOINT=http://...:50051
WEB_BASE_URL=https://your.domain      # share page hosting
YTDL_BIN=yt-dlp
```

---

## Architecture — shared core

```
doradura/
├── crates/
│   ├── core/          # Shared library
│   │   ├── download/  # Pipeline: sources, progress, retry, cleanup
│   │   │   ├── source/ytdlp.rs   YtDlpSource (v5 fallback chain)
│   │   │   ├── source/http.rs    HttpSource (chunked + resume)
│   │   │   ├── pipeline.rs       execute() / download_phase()
│   │   │   └── builder.rs        DownloadRequest builder
│   │   ├── conversion/           FFmpeg wrappers (audio, video, image, doc)
│   │   ├── storage/              SQLite, file management, backup
│   │   ├── lyrics/               LRCLIB + Genius
│   │   ├── odesli/               Streaming link aggregation
│   │   └── metrics/              Prometheus integration
│   │
│   ├── dorabot/       # Telegram bot (doradura v0.13.0)
│   │   ├── telegram/  Bot handlers, menus, callbacks
│   │   ├── audio.rs   Thin wrapper → pipeline::execute()
│   │   └── video.rs   Video pipeline + splitting + burn-in
│   │
│   └── doratui/       # Terminal UI (dora v0.6.6)
│       ├── app.rs     Application state machine
│       ├── main.rs    Event loop, key/mouse handlers
│       ├── settings.rs Persistent settings (JSON)
│       └── ui/        Ratatui renderers (tabs, popups, overlays)
│
├── locales/           Fluent i18n strings (en, ru, fr, de)
├── migrations/        SQLite schema migrations
└── Dockerfile         Multi-stage build (cargo-chef + runtime)
```

---

## Quick Start

### dora TUI

```bash
# macOS
brew tap Jacke/dora && brew install dora

# Ubuntu / Debian
curl -fsSL https://iamjacke.com/doradura/KEY.gpg | sudo gpg --dearmor -o /etc/apt/keyrings/dora.gpg
echo "deb [signed-by=/etc/apt/keyrings/dora.gpg] https://iamjacke.com/doradura stable main" | sudo tee /etc/apt/sources.list.d/dora.list
sudo apt update && sudo apt install dora

# Arch Linux
yay -S dora-bin

# Build from source
git clone https://github.com/Jacke/doradura.git
cd doradura
cargo install --path crates/doratui
```

### doradura Bot (local)

```bash
cp .env.example .env
# Edit .env → add TELOXIDE_TOKEN, TELEGRAM_API_ID/HASH, ADMIN_USERNAME

cargo run -p dorabot -- run
```

### Docker

```bash
docker build -t doradura .
docker run -e TELOXIDE_TOKEN=... -e TELEGRAM_API_ID=... \
           -e TELEGRAM_API_HASH=... -e ADMIN_USERNAME=... \
           -v doradura-data:/data doradura
```

---

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Language | Rust 1.83+ |
| TUI | ratatui 0.30 · crossterm 0.28 |
| Telegram | teloxide 0.17 · local Bot API |
| Async | Tokio (full features) |
| Database | SQLite · rusqlite · r2d2 |
| HTTP | reqwest · SOCKS5 proxy support |
| Media | FFmpeg · yt-dlp (nightly) · Deno |
| i18n | Fluent-templates |
| Metrics | Prometheus |
| Web | Axum · Tower |
| Deploy | Railway · Docker · s6-overlay |
| Theme | Catppuccin Mocha |

---

## Documentation

| Doc | Contents |
|-----|----------|
| [QUICKSTART.md](docs/QUICKSTART.md) | End-to-end setup guide |
| [EXTENDING_SOURCES.md](docs/EXTENDING_SOURCES.md) | Add custom download backends |
| [BOT_FLOWS.md](docs/BOT_FLOWS.md) | Bot conversation state diagrams |
| [LOCAL_BOT_API_SETUP.md](docs/LOCAL_BOT_API_SETUP.md) | Files up to 2 GB via local Bot API |
| [YOUTUBE_COOKIES.md](docs/YOUTUBE_COOKIES.md) | Cookie authentication setup |
| [SUBSCRIPTIONS.md](docs/SUBSCRIPTIONS.md) | Subscription tier management |
| [TESTING.md](docs/TESTING.md) | Test suite & smoke tests |

---

## Testing

```bash
# All tests
cargo test

# Specific crate
cargo test -p dorabot
cargo test -p doratui

# With integration tests
cargo test -- --ignored

# Lint
cargo clippy --workspace
```

---

<div align="center">

MIT License · Made with Rust · [Jacke/doradura](https://github.com/Jacke/doradura)

*Download anything. From anywhere. Beautifully.*

</div>
