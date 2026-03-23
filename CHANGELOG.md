# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- **Tech debt**: Split `shared.rs` (6,920 lines) into `storage/shared/` module with 18 sub-files by domain (task_queue, users, subscriptions, analytics, etc.) — no functional changes
- **Tech debt**: Split `web_server.rs` (3,923 lines) into `core/web/` module with 9 sub-files (public, auth, dashboard, admin_users, admin_queue, admin_errors, admin_misc, types, helpers) — no functional changes
- **Tech debt**: Split `callbacks.rs` (2,176 lines) into 6 sub-modules in `downloads/` (send, clipping, speed, voice_lyrics, categories, cb_helpers) with `CallbackCtx` struct — no functional changes
- **Tech debt**: Split `bot_api_logger.rs` (1,677 lines) via `include!()` — macro-generated `@method` arms moved to `bot_api_logger_methods.rs` — no functional changes

### Added
- **Plan change notifications** — users receive Telegram message whenever their plan changes (admin panel, payment, renewal, cancellation). Event channel between doracore web_server and dorabot dispatcher
- Downloads menu: **Lyrics** button for MP3 — fetches lyrics via Genius/LRCLIB, shows section picker (Verse, Chorus, Bridge…) or full text
- Downloads menu: **Voice** button for MP3 — converts audio to OGG Opus and sends as Telegram voice message
- Downloads menu: **Source link** — clickable `🔗 Source` link to original URL (no preview) in "How to send?" message
- Admin panel: **Queue Monitor** tab — live view of task queue with status filters (active/pending/processing/completed/dead), retry and cancel actions
- Admin panel: **System Health** tab — yt-dlp version, queue breakdown by status, error rate (24h) by type, DB size, unacked alerts/unread feedback counters
- Admin panel: **User Detail** drawer — click any user row to see full profile (subscription, stats, top artists, payments, recent downloads, errors)
- Admin panel: **Feedback Inbox** tab — paginated feedback messages with status filters (new/reviewed/replied), mark-as-read action
- Admin panel: **Alerts** tab — paginated alert history with severity filters (critical/warning/info/unacked), acknowledge action
- Admin panel: **Broadcast** — send message to specific user or broadcast to all from topbar button; fire-and-forget background broadcast with rate limiting
- Admin panel: Errors tab converted from static (last 20) to dynamic API-driven with pagination, resolved/unresolved filter, and per-error resolve button
- Admin API: 13 new endpoints (`/admin/api/queue`, `/admin/api/errors`, `/admin/api/feedback`, `/admin/api/alerts`, `/admin/api/users/{id}/details`, `/admin/api/health`, `/admin/api/broadcast`)
- Admin panel: **Revenue** tab — aggregate stats (total revenue, charges, avg check), revenue-per-day chart, paginated charges table with plan/recurring filters
- Admin panel: **Analytics** API — DAU/WAU/MAU, daily downloads trend, new users per day, format distribution, top users this week (`/admin/api/analytics`)
- Admin panel: Search on Queue, Errors, Feedback, Alerts tabs (debounced, matches URL/message/user/type)
- Admin panel: Error context display — click error row to expand JSON context; `context` field added to errors API
- Admin panel: **Auto-refresh** toggle in topbar — polls active tab every 30s, persists to localStorage
- Admin panel: **User Detail** drawer extended — shows preferences (format, quality, bitrate, burn_subtitles, progress_bar_style), editable plan/language/block via dropdown selectors, block/unblock from drawer
- Admin panel: User settings API (`POST /admin/api/users/{id}/settings`) — update plan (with optional expiry days), language, blocked status
- Admin panel: Broadcast confirmation — "all" target shows `confirm()` dialog before sending
- Admin panel: Feedback reply flow — "Reply" button pre-configures broadcast modal, auto-marks feedback as "replied" after send
- Admin panel: Mobile responsive — tabs scroll horizontally on <768px, detail drawer full-width, toolbar stacks vertically
- Admin panel: **Template extraction** — 1271-line HTML/CSS/JS template moved from inline Rust to `admin_dashboard.html` via `include_str!()`, reducing web_server.rs by ~1265 lines
- Admin panel: **Audit Log** tab — paginated admin action history (plan changes, blocks, broadcasts, settings) with action type filters; V40 migration creates `admin_audit_log` table
- Admin panel: **CSRF protection** — all POST endpoints require `X-CSRF-Token` header validated against session; token embedded in `<meta>` tag and auto-sent by `postJson()`
- Admin panel: **Analytics on Overview** — DAU/WAU/MAU cards, daily active users bar chart, top users this week; loads automatically on page open via `/admin/api/analytics`
- Admin panel: Audit logging in all 9 action handlers (plan, block, retry, cancel, resolve, feedback, ack, broadcast, settings)
- Admin panel: **Content Subscriptions** tab — view all Instagram subscriptions across users with stats (active/inactive/errored/unique sources), status filters, search, enable/disable actions
- Admin panel: **Enhanced Health** — WARP proxy connectivity check, PO Token server check, YouTube cookies validation (per-cookie status for APISID/SAPISID/HSID/SID/SSID), error rate hourly sparkline (24h)
- Admin panel: **Bulk actions** — "Resolve All" button on Errors tab, "Cancel All Pending" on Queue tab; both with confirmation dialogs and audit logging
- Admin panel: **Tab badges** — red badge counters on Queue/Errors/Feedback/Alerts tabs showing active/unresolved/new/unacked counts; polled every 20s via lightweight `/admin/api/counts` endpoint
- Admin API: 3 new endpoints (`/admin/api/errors/bulk-resolve`, `/admin/api/queue/bulk-cancel`, `/admin/api/counts`)

### Changed
- Downloads menu: removed Circle from MP3 (audio-only, no visual), shortened button labels ("Ringtone", "Speed", "Burn subs"), combined Speed+Burn subs in one row for MP4, removed standalone Subtitles button (kept Burn subtitles)
- Split monolithic modules for maintainability: `db/mod.rs` (4909 -> 1617 lines, 8 new modules), `callback_router.rs` (3 files), `admin.rs` (7-file directory module), `commands.rs` (4-file directory module), `downloads.rs` (3-file directory module)

### Fixed
- Health-monitor assumed avatar/name were online when bot was healthy at startup, never re-setting them if a prior rate limit left them stuck on offline. Now always attempts to set online profile on healthy startup
- CSP blocked Telegram OAuth iframe on `/admin/login` — added `frame-src https://oauth.telegram.org` directive
- Degraded video quality when converting circles with speed >1x: `setpts` increased effective FPS (30→45 at 1.5x, 30→60 at 2x), starving the VBV-constrained encoder of bits per frame. Added `fps=30` after `setpts` to normalize output framerate
- Playlist/set URLs produced garbage metadata: yt-dlp `--print` outputs one line per track, but code took all stdout as a single string — titles showed every track name concatenated with newlines
- Added `first_line_of_stdout` helper and `--playlist-items 1` safety net to all 5 metadata `--print` calls
- `sanitize_metadata()` now takes first line only instead of replacing newlines with spaces, and truncates excessively long metadata
- Hardened cache validation to reject multi-line or oversized titles

## [0.31.1] - 2026-03-20

### Fixed
- Download queue completely broken: V19 migration "duplicate column" error caused refinery to roll back entire batch, skipping V39 (task_queue columns). All `save_task_to_queue` and `claim_next_task` calls failed silently
- Pre-apply problematic ALTER TABLE statements from V19/V26 before refinery runs
- `ensure_tables()` now idempotently creates V39 columns on `task_queue` and `processed_updates` table

## [0.31.0] - 2026-03-19

### Added
- Multi-instance runtime with Postgres backend and Redis queue (PR #18)
- `SharedStorage` abstraction — SQLite for dev, Postgres+Redis for production
- `DATABASE_DRIVER` env var to switch between `sqlite` and `postgres`
- Tracing spans with per-task operation IDs for log correlation
- Health monitor crate — auto-recovers bot title, checks `/health`
- Archive ZIP download of user history
- `TempDirGuard` RAII wrapper — eliminates ~40 manual temp file cleanups
- Prometheus `/metrics` endpoint with all download/send/error counters
- Ringtone platform selector (iPhone `.m4r` / Android `.mp3`)

### Changed
- Axum upgraded to 0.8 (path params `{id}` syntax)
- Download module refactored to trait-based `DownloadSource` + `SourceRegistry`

### Fixed
- Axum 0.8 path param syntax (`:id` -> `{id}`) — fixed web server panic
- Tracing subscriber init made non-fatal to prevent crash loops
- Health monitor respects Telegram rate limits, no longer burns `setMyName`
- Archive tables ensured after migration rollback

## [0.30.1] - 2026-03-12

### Fixed
- Dockerfile builder removed from `railway.json`, using GHCR image source
- `set_global_default` + `LogTracer` used separately to avoid log conflict
- `LogTracer::init()` removed — conflicted with tracing-subscriber

## [0.30.0] - 2026-03-10

### Added
- Detailed API logging in health monitor with Retry-After visibility
- URL allowlist enforcement on both preview and download paths

### Fixed
- Health monitor no longer burns `setMyName` rate limit on deploy
- Dependencies updated (quinn-proto CVE, 113 packages)

### Changed
- ~5,400 lines of doracore/dorabot code duplication eliminated

[Unreleased]: https://github.com/Jacke/doradura/compare/v0.31.1...HEAD
[0.31.1]: https://github.com/Jacke/doradura/compare/v0.31.0...v0.31.1
[0.31.0]: https://github.com/Jacke/doradura/compare/v0.30.1...v0.31.0
[0.30.1]: https://github.com/Jacke/doradura/compare/v0.30.0...v0.30.1
[0.30.0]: https://github.com/Jacke/doradura/releases/tag/v0.30.0
