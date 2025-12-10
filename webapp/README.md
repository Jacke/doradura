# Telegram Mini App for Doradura

## What is it?
A Telegram Mini App that runs inside Telegram using the WebApp API, providing a UI to download audio/video via the Doradura bot.

## Features
- 🎵 Audio: MP3 with bitrate selection (128k/192k/256k/320k)
- 🎬 Video: MP4 with quality selection (360p/480p/720p/1080p/best)
- 📝 Subtitles: SRT
- Preview: metadata (title, duration, size) before download
- Subscriptions: Free/Premium/VIP with limits; quick plan switching/cancel to Free
- Limits: show current quotas
- Stats: total downloads, success rate, errors, total size
- History & queue: last 50 downloads with statuses; live queue (poll every 5s) with position/progress
- Settings: send as document/media; persist format/quality; supported services list
- UI: responsive, dark theme, tabs, haptics, Telegram popups
- Security: HMAC-SHA256 validation, freshness check (24h), auth via init data

## Structure
```
webapp/
├── static/
│   ├── index.html    # main page
│   └── app.js        # app logic
└── README.md         # this doc

src/telegram/webapp.rs  # web server
src/main.rs             # bot integration
```

## Setup

### 1) Env vars
Add to `.env`:
```
WEBAPP_PORT=8080              # web server port
WEBAPP_URL=https://your-domain.com  # public HTTPS URL
```
`WEBAPP_URL` must be HTTPS and reachable.

### 2) HTTPS options
- Nginx reverse proxy (recommended)
- ngrok for dev (`ngrok http 8080` and set `WEBAPP_URL` to the ngrok URL)

### 3) Register Mini App in BotFather
1. Open [@BotFather](https://t.me/BotFather)
2. `/mybots` → choose bot → "Bot Settings" → "Menu Button" → "Configure menu button"
3. Button text: `🚀 Mini App`
4. URL: your `WEBAPP_URL`

### 4) Run
```bash
WEBAPP_PORT=8080 cargo run
# or with explicit envs
WEBAPP_PORT=8080 WEBAPP_URL=https://your-domain.com cargo run
```

## How it works (high level)
1. User opens Mini App → Telegram loads `index.html` from the web server.
2. Mini App sends authenticated requests (header `X-Telegram-Init-Data`) to the REST API to load user settings, stats, history, queue.
3. User enters URL, picks format/quality, optionally preview.
4. Mini App POSTs `/api/download`; rate limiter checks limits; task enqueued by plan priority.
5. Queue updates: client polls `/api/user/:id/queue` every 5s; shows status/position/progress.
6. Queue processes: yt-dlp downloads, bot sends file, DB updates status.

## API notes
- All endpoints require `X-Telegram-Init-Data` with valid WebApp init data.
- Server validates HMAC-SHA256 with bot token, freshness (<=24h), extracts `user_id`.
- Error responses include an `error` field; typical HTTP codes: 400/401/404/429/500.

## Dev tips
- For local dev without real HTTPS, use Telegram Desktop + ngrok.
- index.html uses Telegram CSS vars for theming; app.js handles WebApp SDK init, input/URL validation, and requests.
- Backend: `webapp.rs` creates router (`create_webapp_router`, `run_webapp_server`), handles `web_app_data` in dispatcher, creates tasks and enqueues them.

## Debugging
- If Mini App won’t open: ensure `WEBAPP_URL` is HTTPS and correct in BotFather; server running on the right port; check bot logs.
- If data not sent: use browser DevTools → console errors; ensure `Telegram.WebApp` is available; validate JSON payloads.
- CORS issues: `tower-http` CORS is enabled in `webapp.rs`; verify settings/features.

## Resources
See project docs for deployment and API details.
