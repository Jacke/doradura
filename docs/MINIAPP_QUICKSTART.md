# Telegram Mini App Quickstart

## Overview
A basic Telegram Mini App for the Doradura bot with:
- 🎵 MP3 downloads (128k, 192k, 320k)
- 🎬 MP4 downloads (360p, 480p, 720p, 1080p)
- 📝 Subtitle downloads (SRT)
- 🎨 Telegram dark-theme support
- ⚡ Works inside Telegram without opening a browser

## File structure
```
webapp/
├── static/
│   ├── index.html    # Main Mini App page
│   └── app.js        # JavaScript logic
└── README.md         # Detailed docs

src/telegram/webapp.rs  # Backend web server
src/main.rs             # Bot integration
```

## Quick local run

### 1) Install ngrok
```bash
# macOS
brew install ngrok

# Linux
wget https://bin.equinox.io/c/bNyj1mQVY4c/ngrok-v3-stable-linux-amd64.tgz
tar xvzf ngrok-v3-stable-linux-amd64.tgz
sudo mv ngrok /usr/local/bin/
```

### 2) Run the bot with Mini App
In terminal 1:
```bash
cargo build
WEBAPP_PORT=8080 cargo run
```

### 3) Create HTTPS tunnel via ngrok
In terminal 2:
```bash
ngrok http 8080
```
Copy the URL like `https://abc123.ngrok.io`.

### 4) Configure Telegram Mini App
- Set the Mini App URL to the ngrok URL.
- If using Telegram Stars/Payments, configure allowed origins accordingly.

### 5) Test
- Open your bot → launch the Mini App → download a sample audio/video.
- Watch logs for any errors.

For production, replace ngrok with your domain and set `WEBAPP_URL`/`WEBAPP_PORT` env vars in Railway.
