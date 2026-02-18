# User Flow & States

Complete map of all user interaction states in Doradura.

---

## 1. First Contact (`/start` — new user)

```
User sends /start
      │
      ├── Telegram language auto-detected? (ru/en/fr/de)
      │     │
      │     ├─ YES → create user with detected language
      │     │         → show Enhanced Main Menu
      │     │         → send random voice greeting
      │     │
      │     └─ NO  → show Language Selection Menu
      │               [🇷🇺 Russian] [🇺🇸 English]
      │               [🇫🇷 Français] [🇩🇪 Deutsch]
      │                     │
      │                     └── callback: language:select_new:{code}
      │                           → create user with selected language
      │                           → show Enhanced Main Menu
      │
      └── Admin gets notification about new user
```

## 2. Returning User (`/start` — existing user)

```
User sends /start
      │
      └── show Enhanced Main Menu + random voice greeting
```

---

## 3. Enhanced Main Menu (hub)

Shown on `/start` for existing users. Displays current settings summary (format, quality/bitrate, plan).

```
╔══════════════════════════════════════╗
║  Hey! I'm Dora 👋                   ║
║  Format: 🎵 MP3                      ║
║  Bitrate: 320 kbps                   ║
║  Plan: Free                          ║
╠══════════════════════════════════════╣
║  [⚙️ Settings] [📋 Current]          ║
║  [📊 Statistics] [📜 History]        ║
║  [🌐 Services]   [⭐ Subscription]   ║
║  [🌍 Language]   [💬 Feedback]       ║
╚══════════════════════════════════════╝
```

| Button | Callback | Action |
|--------|----------|--------|
| Settings | `main:settings` | → Settings Menu (edit message) |
| Current | `main:current` | → Current Settings Detail (edit message) |
| Statistics | `main:stats` | → delete msg, show user stats |
| History | `main:history` | → delete msg, show download history |
| Services | `main:services` | → Services Menu (edit message) |
| Subscription | `main:subscription` | → delete msg, show subscription info |
| Language | `mode:language` | → Language Menu (edit message) |
| Feedback | `main:feedback` | → delete msg, enter Feedback State |

---

## 4. Settings Menu (`/settings` or `main:settings`)

```
╔══════════════════════════╗
║  [🎬 Quality: 720p]      ║
║  [🎵 Bitrate: 320 kbps]  ║
║  [🌐 Services]            ║
║  [⭐ Subscription]        ║
║  [🌍 Language]            ║
╚══════════════════════════╝
```

| Button | Callback | Submenu |
|--------|----------|---------|
| Video Quality | `mode:video_quality` | Video Quality Menu |
| Audio Bitrate | `mode:audio_bitrate` | Audio Bitrate Menu |
| Services | `mode:services` | Services/Extensions List |
| Subscription | `mode:subscription` | Subscription Info |
| Language | `mode:language` | Language Menu |

### 4a. Video Quality Menu

```
[🎬 Best ✓] [1080p] [720p] [480p] [360p]
[⬅️ Back]
```

Callbacks: `quality:{value}` — saves to DB, refreshes menu. Back: `back:main`.

### 4b. Audio Bitrate Menu

```
[128 kbps] [192 kbps] [256 kbps] [320 kbps ✓]
[⬅️ Back]
```

Callbacks: `bitrate:{value}`. Back: `back:main`.

### 4c. Language Menu

```
[🇷🇺 Russian ✓] [🇺🇸 English]
[🇫🇷 Français]   [🇩🇪 Deutsch]
[⬅️ Back]
```

Callbacks: `language:set:{code}` (existing user) or `language:select_new:{code}` (new user).

### 4d. Services Menu

Lists extensions from `ExtensionRegistry` grouped by category (Download Sources, Converters, etc.).

```
[🎵 YouTube Music]  → ext:detail:youtube_music
[📱 TikTok]         → ext:detail:tiktok
...
[⬅️ Back]           → back:enhanced_main
```

Extension detail: `ext:detail:{id}` → shows description, capabilities, examples. Back: `ext:back`.

---

## 5. URL Download Flow

```
User sends URL (e.g., https://youtube.com/watch?v=...)
      │
      ├── 👀 reaction set on message
      │
      ├── Rate limit check
      │     ├─ BLOCKED → "Please wait {N} seconds"
      │     └─ OK → continue
      │
      ├── URL validation (length ≤ 2048, parseable)
      │     └─ FAIL → "Invalid link"
      │
      ├── Get user preferences (format, quality, bitrate, plan)
      │
      ├── Single URL → Preview with metadata
      │     │
      │     │   ╔══════════════════════════════════╗
      │     │   ║  🎵 Song Title                    ║
      │     │   ║  Artist · 3:42 · 8.5 MB           ║
      │     │   ║  [thumbnail image]                 ║
      │     │   ╠══════════════════════════════════╣
      │     │   ║  [⬇️ MP3]  [🎬 MP4]  [🎬🎵 Both] ║
      │     │   ║  [⚙️ Settings]  [❌ Cancel]        ║
      │     │   ║  [📹 Media ✓]  ← toggle doc/media ║
      │     │   ╚══════════════════════════════════╝
      │     │
      │     ├── dl:{format}:{url_id}           → start download
      │     ├── dl:{format}:{quality}:{url_id} → start download with quality
      │     ├── pv:cancel:{url_id}             → delete preview
      │     ├── pv:set:{url_id}                → open settings from preview
      │     ├── video_send_type:toggle:{url_id} → toggle media/document mode
      │     └── mode:*:preview:{url_id}        → settings submenus (return to preview)
      │
      └── Multiple URLs → Group download
            │
            └── All URLs queued at once, status message updated per-URL
```

### 5a. Download Queue States

```
Task added to queue
      │
      ├── Queue empty → processing immediately
      │     └── "Task added, processing..."
      │
      └── Queue has items → show position
            └── "Queue position: {pos}/{total}"
                  └── queue > 5 && free plan → "Want to skip the queue? /plan"
```

Priority: Free=0, Premium=70, VIP=100 (higher priority = processed first).

### 5b. Download Processing

```
Queue processes task
      │
      ├── yt-dlp fallback chain:
      │     1. No cookies (android_vr + web_safari clients)
      │     2. With cookies + PO token
      │     3. Fixup never (last resort)
      │
      ├── Progress updates → edit status message periodically
      │
      ├── SUCCESS
      │     ├── Audio → send audio file + effects button
      │     │     └── [🎛 Effects] → Audio Effects Menu (ae:*)
      │     │
      │     └── Video → send video + optional subtitle burning
      │           ├── Large video → split into parts
      │           └── Saved to download history
      │
      └── FAILURE
            ├── Size too large → "File is too large"
            ├── Rate limited by source → "Try again later"
            ├── Not found → "Video not found"
            └── Generic error → sanitized error message
                  └── Admin gets error notification
```

---

## 6. File Upload Flow (media sent to bot)

```
User sends photo/video/audio/document
      │
      ├── Cookies upload session active?
      │     └─ YES → handle as cookies file (admin flow)
      │
      ├── Save to uploads DB (title, file_id, size, type, dimensions, duration)
      │
      └── Show Level 1 Action Menu
            │
            │   ╔══════════════════════════════════╗
            │   ║  🎬 Video Title                    ║
            │   ║  └ 50.5 MB · 2:30 · 1920x1080     ║
            │   ╠══════════════════════════════════╣
            │   ║  [📤 Send] [🔄 Convert]            ║  ← video
            │   ║  [🗑️ Delete]  [📂 All uploads]     ║
            │   ╚══════════════════════════════════╝
            │
            │   Photo/Audio:  [📤 Send] / [🗑️ Delete] / [📂 All uploads]
            │   Document:     [📤 Send] → direct send / [🗑️ Delete]
            │
            ├── videos:submenu:send:{id} → Level 2 Send Menu
            │     ╔════════════════════════════╗
            │     ║  📤 Send Video Title:        ║
            │     ║  [📤 Video]  [📎 Document]   ║  ← video
            │     ║  [📤 Photo]  [📎 Document]   ║  ← photo
            │     ║  [📤 Audio]  [📎 Document]   ║  ← audio
            │     ║  [⬅️ Back]                   ║
            │     ╚════════════════════════════╝
            │
            ├── videos:submenu:convert:{id} → Level 2 Convert Menu
            │     ╔════════════════════════╗
            │     ║  🔄 Convert:            ║
            │     ║  [⭕ Circle] [🎵 MP3]   ║
            │     ║  [🎞️ GIF]  [📦 Compress]║
            │     ║  [⬅️ Back]               ║
            │     ╚════════════════════════╝
            │
            ├── videos:send:{type}:{id}     → send file as video/document/photo/audio
            ├── videos:delete:{id}          → delete from DB + confirm
            ├── videos:open:{id}            → back to Level 1 (edit message)
            └── convert:*                   → conversion handlers
```

### 6a. Video Circle Conversion (`convert:circle:{id}`)

```
Circle selected
      │
      ├── Video duration > 60s?
      │     └── Show duration picker
      │           ╔═══════════════════════════════╗
      │           ║  [▶ 0:00–0:15] [0:00–0:30] [0:00–1:00]  ║
      │           ║  [◀ ...–0:15]  [...–0:30]  [...–1:00]    ║
      │           ║  [🔄 Middle]  [📐 Full]                   ║
      │           ║  [⬅️ Back]                                ║
      │           ╚═══════════════════════════════╝
      │           Callbacks: videos:dur:{range_type}:{id}:{seconds}
      │
      ├── Video needs splitting? (multi-part circles)
      │     └── Split into ≤60s parts, send sequentially
      │
      └── Process with FFmpeg → send as video_note
```

---

## 7. Downloads History (`/downloads`)

```
/downloads [mp3|mp4|search_query]
      │
      ├── Empty → "No downloads"
      │
      └── Paginated list (5 per page)
            │
            │   ╔══════════════════════════════════╗
            │   ║  📥 Your downloads                ║
            │   ║                                    ║
            │   ║  1. 🎵 Song Title                  ║
            │   ║     └ MP3 · 5.2 MB · 3:42          ║
            │   ║  ...                               ║
            │   ╠══════════════════════════════════╣
            │   ║  [🎵] [🎬] [📋 All]     ← filters  ║
            │   ║  [⬅️ 1/3] [➡️]          ← pages    ║
            │   ╚══════════════════════════════════╝
            │
            ├── downloads:page:{n}:{filter}:{search}  → navigate pages
            ├── downloads:open:{id}                    → show download detail
            │     │
            │     ├── [📤 Resend]      → downloads:resend:{id}
            │     ├── [⭕ Circle]      → duration picker (downloads:dur:*)
            │     ├── [✂️ Clip]        → start Video Clip Session
            │     ├── [🗑️ Delete]     → downloads:delete:{id}
            │     └── [⬅️ Back]       → downloads:back:{page}
            │
            └── downloads:filter:{type}               → filter by mp3/mp4/all
```

---

## 8. Uploads (`/uploads`)

```
/uploads [video|photo|document|audio|search_query]
      │
      └── Paginated list (same as /downloads but for uploaded files)
            │
            ├── videos:page:{n}:{filter}:{search}  → navigate pages
            ├── videos:open:{id}                    → Level 1 action menu
            └── Filter buttons by media type
```

---

## 9. Cuts (`/cuts`)

```
/cuts
      │
      ├── Empty → "No clips. Open /downloads and press ✂️"
      │
      └── Paginated list of created cuts
            │
            ├── cuts:page:{n}                      → navigate pages
            ├── cuts:open:{id}                     → show cut detail
            │     ├── [📤 Resend]                  → cuts:resend:{id}
            │     ├── [⭕ Circle]                   → duration picker (cuts:dur:*)
            │     ├── [✂️ New clip]                → start new clip session from cut
            │     ├── [🗑️ Delete]                 → cuts:delete:{id}
            │     └── [⬅️ Back]                   → cuts:back:{page}
            │
            └── cuts:dur:{range}:{id}:{seconds}   → circle from cut
```

---

## 10. Video Clip Session (interactive)

Activated by pressing "✂️ Clip" on a download or cut.

```
Session started
      │
      ├── Bot sends prompt:
      │     "Send intervals in mm:ss-mm:ss format"
      │     "Multiple intervals separated by comma: 00:10-00:25, 01:00-01:10"
      │     "Or: full, first30, last30, middle30"
      │     "Speed: first30 2x, full 1.5x"
      │     "Type cancel to exit"
      │
      ├── User sends text:
      │     ├── "cancel" → session deleted, "Cancelled"
      │     ├── valid intervals     → process clip with FFmpeg
      │     │     └── segments extracted → concatenated → sent as video
      │     └── invalid format      → "Could not parse intervals" + format hint
      │
      └── Session expired → "Session expired"
```

---

## 11. Audio Cut Session (interactive)

Activated by "✂️ Cut" button on audio effects menu.

```
Session started (from audio effects ae:cut:{session_id})
      │
      ├── Bot sends prompt with audio duration info
      │
      ├── User sends intervals (same format as video clips)
      │     ├── "cancel" → cancelled
      │     ├── valid    → extract + send audio segments
      │     └── invalid  → error + retry
      │
      └── Session expired → "Session expired"
```

---

## 12. Audio Effects (`ae:*`)

Shown after successful audio download.

```
[🎛 Effects] button on downloaded audio
      │
      └── ae:menu:{session_id}
            │
            ╔══════════════════════════════════╗
            ║  [🔊 Bass Boost]  [⏩ Speed Up]   ║
            ║  [🔽 Slow Down]   [🎵 Pitch Up]   ║
            ║  [🎵 Pitch Down]  [📱 Ringtone]   ║
            ║  [✂️ Cut]                          ║
            ╚══════════════════════════════════╝
            │
            ├── ae:bass:{id}      → bass boost + send
            ├── ae:speed_up:{id}  → speed up + send
            ├── ae:slow_down:{id} → slow down + send
            ├── ae:pitch_up:{id}  → pitch up + send
            ├── ae:pitch_down:{id}→ pitch down + send
            ├── ae:ringtone:{id}  → create ringtone + send
            └── ae:cut:{id}       → start Audio Cut Session
```

---

## 13. Audio Cut Callbacks (`ac:*`)

```
ac:start:{download_id}         → start audio cut from download
ac:apply:{session_id}:{range}  → apply specific cut
```

---

## 14. Feedback State

```
main:feedback clicked
      │
      ├── Bot: "Write your feedback..."
      │     └── FEEDBACK_STATES[user_id] = true
      │
      ├── User sends any text (not a command)
      │     ├── Save feedback to DB
      │     ├── Notify admin with user info + message
      │     ├── Bot: "Thanks for the feedback!"
      │     └── FEEDBACK_STATES[user_id] = false
      │           └── show Enhanced Main Menu
      │
      └── User sends a command → exits feedback state implicitly
```

---

## 15. Subscription & Payments (`/plan`)

```
/plan or main:subscription
      │
      ├── Show current plan info + available plans
      │     ╔══════════════════════════════════╗
      │     ║  📋 Your plan: Free               ║
      │     ║                                    ║
      │     ║  ⭐ Premium — {price} Stars/mo    ║
      │     ║  👑 VIP — {price} Stars/mo        ║
      │     ╠══════════════════════════════════╣
      │     ║  [⭐ Premium]  [👑 VIP]           ║
      │     ║  [❌ Cancel subscription]         ║ ← if subscribed
      │     ╚══════════════════════════════════╝
      │
      ├── subscribe:{plan}  → create Telegram Stars invoice
      │     │
      │     ├── PreCheckoutQuery → validate payload → approve
      │     │
      │     └── SuccessfulPayment
      │           ├── Activate subscription in DB
      │           ├── Bot: "Subscription activated!"
      │           └── Admin notification
      │
      └── subscription:cancel → cancel subscription
            └── "Subscription cancelled. Active until end of period."
```

---

## 16. Download History (`/history`)

```
/history or main:history
      │
      └── Paginated download history
            │
            ├── history:page:{n}               → navigate
            ├── history:redownload:{id}        → re-add to queue
            └── history:delete:{id}            → remove from history
```

---

## 17. Export (`/export`)

```
/export
      │
      └── Format selection
            ├── export:txt   → export history as TXT
            ├── export:csv   → export history as CSV
            └── export:json  → export history as JSON
```

---

## 18. Info Command (`/info <URL>`)

```
/info https://youtube.com/...
      │
      ├── Fetch metadata via yt-dlp --dump-json
      │
      ├── Show available formats:
      │     "🎬 Video: 1080p (50MB), 720p (25MB), 480p (12MB)"
      │     "🎵 Audio: MP3 320kbps (8MB), 192kbps (5MB)"
      │
      └── No URL provided → "Provide a URL after /info"
```

---

## 19. Downsub (`/downsub`)

```
/downsub summary <URL>  → get AI summary via Downsub gRPC
/downsub subtitles <URL> → fetch subtitles via Downsub gRPC
/downsub                → show usage help
```

---

## 20. Preview Settings from Preview

When user clicks "⚙️ Settings" on a URL preview, settings carry the `url_id` context so they can return to the preview:

```
pv:set:{url_id}
      │
      └── Settings Menu with preview context
            │
            ├── mode:download_type:preview:{url_id}    → format selection
            ├── mode:video_quality:preview:{url_id}    → quality selection
            ├── mode:audio_bitrate:preview:{url_id}    → bitrate selection
            │
            └── Changing format auto-starts download:
                  format:{fmt}:preview:{url_id}:{preview_msg_id}
                        → start_download_from_preview()

Back navigation:
      back:preview:{url_id}                  → return to preview
      back:main:preview:{url_id}:{msg_id}    → return to settings menu
```

---

## 21. Admin Flows (hidden commands)

All admin commands check `is_admin(user_id)` before executing.

### Visible admin commands (in Command enum)
| Command | Action |
|---------|--------|
| `/admin` | Admin panel with inline buttons |
| `/backup` | Create and send SQLite backup |
| `/users` | List all users with stats |
| `/setplan {user_id} {plan}` | Change user's subscription plan |
| `/transactions` | View Telegram Stars transactions |
| `/charges` | View all payment charges |
| `/download_tg {file_id}` | Download file from Telegram by file_id |
| `/sent_files` | List recently sent files with file_ids |
| `/analytics` | Analytics dashboard |
| `/health` | System health check |
| `/downsub_health` | Downsub gRPC connection check |
| `/metrics` | Detailed system metrics |
| `/revenue` | Financial analytics |
| `/botapi_speed` | Local Bot API speed test |
| `/version` | Show version + yt-dlp version + update button |

### Hidden admin commands (not in Command enum, matched by text filter)
| Command | Callback | Action |
|---------|----------|--------|
| `/update_cookies` | — | Start cookies update flow |
| `/diagnose_cookies` | — | Check cookies file validity |
| `/update_ytdlp` | — | Update yt-dlp binary |
| `/browser_login` | — | Start browser-based YouTube login |
| `/browser_status` | — | Check browser session status |

### Admin panel callbacks (`admin:*`)
```
admin:browser_*          → browser/cookie management
admin:check_ytdlp        → check yt-dlp version
admin:update_ytdlp       → update yt-dlp
admin:setplan:{user_id}:{plan} → change user plan
```

### Analytics callbacks
```
analytics:refresh   → refresh analytics dashboard
analytics:details   → show metrics categories
analytics:close     → delete analytics message

metrics:performance → performance metrics detail
metrics:business    → business metrics detail
metrics:engagement  → engagement metrics detail
```

---

## Callback Prefix Reference

| Prefix | Handler | Description |
|--------|---------|-------------|
| `ac:` | `handle_audio_cut_callback` | Audio cut operations |
| `ae:` | `handle_audio_effects_callback` | Audio effects (bass, speed, pitch, ringtone) |
| `mode:` | inline in `handle_menu_callback` | Settings submenus |
| `main:` | inline in `handle_menu_callback` | Enhanced main menu actions |
| `ext:` | inline in `handle_menu_callback` | Extension/service details |
| `subscribe:` | inline in `handle_menu_callback` | Start subscription payment |
| `subscription:` | inline in `handle_menu_callback` | Manage subscription |
| `video_send_type:` | inline in `handle_menu_callback` | Toggle media/document send mode |
| `back:` | inline in `handle_menu_callback` | Navigation back |
| `format:` | inline in `handle_menu_callback` | Set download format |
| `dl:` | inline in `handle_menu_callback` | Start download from preview |
| `pv:` | inline in `handle_menu_callback` | Preview actions (cancel, settings) |
| `history:` | `handle_history_callback` | Download history navigation |
| `export:` | `handle_export` | Export history |
| `analytics:` | inline in `handle_menu_callback` | Admin analytics |
| `metrics:` | inline in `handle_menu_callback` | Admin detailed metrics |
| `downloads:` | `handle_downloads_callback` | Downloads list + actions |
| `cuts:` | `handle_cuts_callback` | Cuts list + actions |
| `videos:` | `handle_videos_callback` | Uploads list + Level 1/2 menus |
| `convert:` | `handle_videos_callback` | Conversion actions on uploads |
| `admin:` | inline in `handle_menu_callback` | Admin panel actions |
| `language:` | inline in `handle_menu_callback` | Language selection |
| `quality:` | inline in `handle_menu_callback` | Video quality selection |
| `bitrate:` | inline in `handle_menu_callback` | Audio bitrate selection |

---

## Handler Priority (dptree order)

The handler chain in `schema()` processes updates in this order — first match wins:

1. **Successful payment** — `msg.successful_payment().is_some()`
2. **`/update_cookies`** — hidden admin command
3. **`/diagnose_cookies`** — hidden admin command
4. **`/update_ytdlp`** — hidden admin command
5. **`/browser_login`** — hidden admin command
6. **`/browser_status`** — hidden admin command
7. **Bot commands** — `/start`, `/settings`, `/info`, etc.
8. **Media upload** — photo/video/audio/document sent to bot
9. **Message handler** — URLs, text (audio cut sessions, video clip sessions, feedback, link processing)
10. **Pre-checkout query** — Telegram Stars payment validation
11. **Callback query** — all inline button clicks (routed by prefix)

---

## Text Input States (non-command messages)

When the user sends plain text (not a command, not a URL), the system checks these states in order:

1. **Cookies upload session** — if active, treat document as cookies file
2. **Audio cut session** — if active, parse as time intervals
3. **Video clip session** — if active, parse as time intervals + optional speed
4. **Feedback state** — if waiting, treat text as feedback message
5. **URL detection** — regex match for `https?://` links
6. **No match** — message ignored

---

## Error States

| Error | User Sees | Admin Sees |
|-------|-----------|------------|
| Rate limited | "Please wait {N} seconds" | — |
| URL too long | "URL is too long" | — |
| Invalid URL | "Invalid link" | — |
| File too large | "File is too large ({size})" | — |
| Source not found | "Video not found" | — |
| yt-dlp error | Sanitized error message | Full error + stack trace |
| DB connection fail | Generic error | Logged |
| Upload expired | "File not found" | — |
| URL cache expired | "Link expired, please send again" | — |
| Session expired | "Session expired" | — |
| Payment error | "Error creating invoice" | Error details |
| FFmpeg failure | "Conversion error" | Full error logged |
| Conversion timeout | (currently no timeout) | — |
