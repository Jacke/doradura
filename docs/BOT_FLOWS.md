# Bot Flows and States

This document describes the Doradura Telegram bot architecture, states, and processing flows.

## Contents
1. Architecture overview
2. System states
3. Processing flows
4. Configuration highlights
5. Example journeys

---

## 1. Architecture overview
- **main.rs** — entry point, initialization, dispatcher.
- **commands.rs** — command and message handling.
- **menu.rs** — inline menus and callback handling.
- **downloader.rs** — audio/video/subtitle download logic.
- **queue.rs** — prioritized download queue.
- **rate_limiter.rs** — per-user throttling.
- **db.rs** — SQLite storage (users, history, tasks).
- **progress.rs** — progress tracking and UI.
- **config.rs** — configuration constants.
- **utils.rs** — helpers.

Data flow: User → Telegram API → Dispatcher → Handlers → Queue/DB → Downloader → Telegram API (file send).

---

## 2. System states
1. **Initialization** — load env, connect DB, set up dispatcher and queue.
2. **Waiting for updates** — long polling (or webhook if configured).
3. **Command handling** — `/start`, `/mode`, `/plan`, etc.
4. **Message handling** — URL parsing/validation, enqueue tasks.
5. **Rate limit check** — throttle per user/plan.
6. **Menu state** — inline menu interactions.
7. **Queue processing** — pull tasks and download.
8. **Download** — audio/video/subtitles.
9. **Send file** — upload to Telegram.
10. **Error** — notify user/admin, retry when appropriate.

Progress states: Starting → Downloading → Uploading → Success/Completed → Error.

---

## 3. Processing flows

### 3.1 Initialization
- Configure logging.
- Load env (`TELOXIDE_TOKEN`, cookies path, etc.).
- Initialize bot, DB pool, rate limiter, and empty queue.
- Register command handlers and menu callbacks.
- Start queue worker (Tokio task) and optional Mini App server.
- Dispatcher runs with retry/backoff on panic (`TX is dead` guard).

### 3.2 `/start`
- Send random sticker (from pack `doraduradoradura`).
- Send greeting message: "Give me a link and I’ll download it".
- Transition to waiting state.

### 3.3 `/mode`
- Fetch user settings (format/quality/bitrate).
- Render inline menu with options:
  - Download type (mp3/mp4/srt/txt/mp4+mp3)
  - Video quality or audio bitrate
  - Services info
  - Subscription info
- Update settings on callback selection.

### 3.4 Message with URL
1. Ignore `/start`/`/help` text.
2. Extract URL via regex; validate length and syntax.
3. Strip playlist param `list` from YouTube links.
4. Load user format settings; default `mp3` if absent.
5. Check rate limit (plan-based interval).
6. Create `DownloadTask` (uuid, format, quality/bitrate, timestamps).
7. Add task to queue; log request to DB.
8. Respond with confirmation or error messages for invalid/long URL or rate limit.

### 3.5 Callback handling (menus)
- Format selection (`format:mp3|mp4|srt|txt|mp4+mp3`).
- Services info.
- Back to main menu.
- Download from preview with chosen format/quality (`dl:...`).
- Subscription actions (open info, cancel, pay via Stars).
- Admin actions (list users, change plan, manage user).

### 3.6 Queue processing
- Runs in a loop with interval; semaphore limits concurrency (default 5).
- Pop highest-priority task (VIP > Premium > Free).
- Mark task as processing in DB.
- Parse URL; on failure mark failed and notify admin.
- Dispatch to downloader based on format:
  - `mp4` → `download_and_send_video`
  - `srt`/`txt` → `download_and_send_subtitles`
  - otherwise → `download_and_send_audio`
- On success: mark completed. On error: mark failed, optionally notify admin if retries remain.

### 3.7 Audio download flow
1. Create progress message.
2. Fetch metadata (timeout 120s); show "Starting".
3. Generate safe filename; choose bitrate.
4. Run yt-dlp with cookies/client selection.
5. Parse progress from stdout; update UI to 100%.
6. Validate file size (<49 MB default for audio).
7. Send audio with retry (max 3), showing uploading status.
8. Auto-delete temp file after delay.
9. On error: send sticker + friendly error text; mark failed.

### 3.8 Video download flow
- Similar to audio; choose quality (`best/1080p/720p/480p/360p`).
- Simulated progress 10%→90% while downloading.
- Validate size (<49 MB default for video).
- Send via `send_video` with retries; auto-clean temp file.

### 3.9 Subtitles flow (SRT/TXT)
- Use yt-dlp with `--skip-download` and `--convert-subs`.
- Send as document; single attempt.
- Validate existence of subtitle file; handle errors with sticker + message.

### 3.10 Rate limiting
- Per-user timestamps with 30s base interval for Free, shorter for paid plans.
- Inform user how long to wait; store limits in memory with auto-expiry.

### 3.11 Error handling
- Centralized messages for invalid URL, size too large, yt-dlp failures, send failures, menu DB errors.
- Admin notification on task failures (with URL, user, error preview) when retries remain.
- Detailed logging with levels warn/error.

---

## 4. Configuration highlights
- Rate limit interval: 30s (Free), shorter for Premium/VIP.
- Max concurrent downloads: 5 (semaphore).
- Queue check interval: 100 ms.
- File size limits: default 49 MB audio/video (configurable).
- HTTP timeout: 300s.
- Progress animation interval: 500 ms.
- URL max length: 2048 chars.

---

## 5. Example journeys

### Successful audio download
1. User sends YouTube link.
2. Rate limit passes; task enqueued.
3. Progress messages show Starting → percentage → Uploading → Success.
4. Audio file sent; message cleaned after delay; temp file removed.

### Rate-limited request
1. User sends link too soon.
2. Bot replies with wait time; task not enqueued.

### Change download format
1. User opens `/mode` and picks MP4.
2. Setting stored in DB.
3. Next URL uses MP4; queue processes video flow.

### Error case
1. yt-dlp fails (video unavailable).
2. Bot sends error sticker + message; task marked failed; admin notified if retries remain.

---

This overview captures the primary flows, states, and safeguards that power Doradura's Telegram bot.
