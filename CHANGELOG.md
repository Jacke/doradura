# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- **`BotExt` extension trait for MarkdownV2 send/edit chains** (v0.36.11) — new `crates/dorabot/src/telegram/ext.rs` module with four methods (`send_md`, `send_md_kb`, `edit_md`, `edit_md_kb`) that collapse the repetitive

  ```rust
  bot.send_message(chat_id, text)
      .parse_mode(ParseMode::MarkdownV2)
      .reply_markup(kb)
      .await
  ```

  into a single `bot.send_md_kb(chat_id, text, kb).await` call. Migrated **55 call sites across 14 files**: `analytics.rs`, `cuts.rs`, `feedback.rs`, `videos.rs`, `preview/vlipsy.rs`, `handlers/uploads.rs`, `admin/{users, cookies, system, browser}.rs`, `menu/{lyrics, ringtone, callback_admin, vlipsy}.rs`. The trait is `async fn in trait` (stable in Rust 1.75+, no `async_trait` needed) and delegates to the existing teloxide builder — zero new state, zero allocations. Removed now-unused `ParseMode` imports from 3 files

- **Typed JSON envelopes in admin handlers** (v0.36.10) — replaced 18 stringly-typed `Json(json!({"ok": true, ...}))` sites across `admin_errors.rs`, `admin_queue.rs`, `admin_misc.rs`, `admin_users.rs`, and `public.rs` with 12 new `#[derive(Serialize)]` structs in `core/web/types.rs`: `OkResponse`, `ErrorResponse`, `RetryOk`, `NotifyOk`, `BulkCountOk`, `PlanChangeOk`, `BlockOk`, `SettingsUpdatedOk`, `FeedbackStatusOk`, `BroadcastSingleOk`, `BroadcastStartOk`, `ToggleOk`. The wire-format JSON shape is **bytewise-identical** to what the admin SPA already consumes; the change is purely compile-time schema enforcement. Complex nested `json!(...)` builders in `admin_users::admin_api_user_details` and the dashboard stats stay as-is — those are genuine dynamic JSON, not envelope shapes

- **`once_cell::sync::Lazy` → `std::sync::LazyLock` migration** (v0.36.9) — the codebase had 37 `once_cell::sync::Lazy` sites across 14 files mixed with 13 existing `std::sync::LazyLock` sites (inconsistent). `LazyLock` has been stable in stdlib since Rust 1.80, so the `once_cell` crate is no longer needed for this. Migrated all 37 sites and **removed `once_cell` as a direct dependency** from all three crates (`doracore`, `dorabot`, `doratui`) and from the workspace root. It may still appear transitively via `fluent-templates` / `regex` etc., but it's no longer in our own Cargo.tomls. Net: one fewer dep to compile, one fewer API to remember, one consistent pattern across the whole workspace

- **`Result<T, String>` → `anyhow::Result<T>` rollout** (v0.36.8) — migrated **40+ function signatures** across 20+ files from the lazy `Result<T, String>` escape hatch to proper `anyhow::Result<T>`. Callers now get:
  - Error source chains preserved (`.source()` walks the underlying error)
  - `.with_context(|| "...")` for contextual layering instead of manual `format!` wrapping
  - `?`-propagation across error types without boilerplate conversion
  - Consistent error type across subsystems that previously returned String

  Files touched: `core/alerts.rs` (12 methods), `watcher/{db,traits,scheduler,instagram}.rs`, `core/subscription.rs`, `core/stats_reporter.rs`, `telegram/{menu/vault, menu/archive, admin/browser, admin/cookies, downloads/subtitles, menu/callback_settings}.rs`, `download/{search, pipeline, playlist_import}.rs`, `download/playlist_sync/{mod, resolver, spotify, soundcloud, yandex_music}.rs`, `vlipsy.rs`, `doracore/download/{cookies, playlist}.rs`, `doracore/core/web/admin_errors.rs`, `doratui/{video_info, download_runner}.rs`. Also updated the `DiskAlertFn` type alias in `doracore/core/disk.rs`.

  Internal `.map_err(|e| format!(...))` patterns converted to `.with_context(|| "...")`. Inline `return Err("literal".to_string())` converted to `anyhow::bail!("literal")`. Ocassional `e.to_string().contains(...)` patches added where a caller was doing string-matching on what used to be a raw error string.

- **`strum` rollout extended** (v0.36.8) — `OutputKind` and `SourceKind` in `doracore/storage/db/sessions.rs` (missed in Batch B because they live inside the `CutEntry` struct whose other fields made it a skip-candidate for FromRow) now derive `strum::Display` + `strum::AsRefStr` + `strum::IntoStaticStr`. Their manual `fmt::Display` impls and positional match blocks are gone; `as_str()` is aliased to `Into::<&'static str>::into`. `from_str_lossy` stays manual because it has a "fall back to Cut on unknown input" contract strum's `EnumString` doesn't express.

- **`pretty_assertions` added as a dev-dependency** (v0.36.7) — nicer colorized diffs on `assert_eq!` failures in tests. Opt-in per test module via `use pretty_assertions::assert_eq;`, no runtime cost, no production impact

- **`strum` derive rollout** (v0.36.6) — added `strum = "0.26"` (derive feature) and replaced hand-written `impl Display` / `impl FromStr` / `as_str()` match blocks on six enums with derive macros:
  - `Plan` (doracore/core/types.rs) — full rollout: `strum::Display` + `strum::EnumString` + `strum::AsRefStr` + `strum::IntoStaticStr` with `serialize_all = "lowercase"`. `as_str()` kept as a one-line alias for `Into::into` so existing call sites don't change. Removed manual `FromStr`, `Display`, and the duplicated match in `as_str`
  - `DownloadFormat` (dorabot/download/queue.rs) — same full rollout as `Plan`
  - `PlanChangeReason` (doracore/core/types.rs) — `strum::Display` only
  - `MorphProfile` (doracore/download/audio_effects.rs) — `strum::Display` + `AsRefStr` + `IntoStaticStr`. `FromStr` kept manual because the enum has a "fall back to `None` on unknown input" contract (`Err = Infallible`) that strum's `EnumString` doesn't express
  - `SmokeTestStatus` (dorabot/smoke_tests/results.rs) — `strum::Display` with `serialize_all = "UPPERCASE"`
  - `Platform` (dorabot/download/playlist_sync/resolver.rs) — `strum::Display` with per-variant `serialize` attributes (the human labels have spaces, e.g. `"Yandex Music"`). `db_name()` kept manual as it's a separate snake_case representation
  - `TrackStatus` (same file) — `strum::AsRefStr` + `IntoStaticStr`
  - `ProxyProtocol` (doracore/download/proxy.rs) — `strum::Display` with `serialize_all = "lowercase"`
  - Net: ~100 LOC deleted, all 560 tests pass (Plan's 8 existing unit tests validate the behavioral equivalence of the derived impls)

- **`fluent_args!` macro + centralized `format_bytes`** (v0.36.5):
  - New `doracore::fluent_args!` macro replaces the repeated `let mut args = FluentArgs::new(); args.set("k1", v1); args.set("k2", v2);` ceremony at 58 call sites across 15 files. Usage: `let args = doracore::fluent_args!("count" => n, "name" => username);` (trailing commas allowed, inside doracore itself use `crate::fluent_args!`)
  - New `doracore::core::format_bytes(u64)` / `format_bytes_i64(i64)` helpers replace 7 duplicated `format_file_size` / `format_size` / `format_bytes` / `fmt_size` functions scattered across `core/stats.rs`, `core/stats_reporter.rs`, `telegram/preview/display.rs`, `telegram/downloads/mod.rs`, `telegram/videos.rs`, `telegram/cuts.rs`, `telegram/menu/archive.rs`, and `doratui/src/video_info.rs`. Each file now just re-exports the canonical helper under its local name. Added TB handling (old helpers topped out at GB and would have shown "1024.00 GB" for 1.5 TB files)
  - Evaluated `humansize` crate for format_bytes but dropped it — its default output is SI-style "1 kB" (lowercase k) which doesn't match the user-visible "1 KB" users see today. The 10-line custom helper preserves the exact existing format
  - Net: ~160 LOC deleted, zero user-visible behavior change, 6 new tests

- **`#[derive(sqlx::FromRow)]` rollout** (v0.36.4) — enabled the `sqlx` `macros` feature and replaced hand-written `map_pg_*` helpers with `#[derive(sqlx::FromRow)]` for the three structs whose Postgres columns map 1:1 to fields without any bool-as-i32 / enum-as-string / JSON parsing quirks: `SharePageRecord`, `PlaylistItem`, `SyncedTrack`. Call sites switched from `sqlx::query(...).fetch_*(...)` + manual `.map(map_pg_...)` to `sqlx::query_as::<_, T>(...).fetch_*(...)`. The remaining 14 `map_pg_*` helpers (Charge, DownloadHistoryEntry, ErrorLogEntry, Playlist, SubtitleStyle, etc.) are intentionally kept — they do real conversion work (bool↔i32, enum parsing, JSON decoding) that isn't mechanically expressible via `FromRow` attributes and would regress readability to port. ~45 LOC deleted, zero behavior change

### Fixed
- **Subprocess zombie leak on timeout** (v0.36.3) — every inline `tokio::time::timeout(dur, cmd.output())` call site across the codebase was missing `cmd.kill_on_drop(true)`. When the timeout fired, the tokio future was dropped but the subprocess kept running until it finished naturally — ffmpeg/LibreOffice can easily hold CPU, RAM, file handles, and worker slots for many minutes past the nominal timeout. Added a new `core::process::run_with_timeout_raw(cmd, dur) -> Result<io::Result<Output>, Elapsed>` helper that always sets `kill_on_drop` and exposes the raw nested result so callers can keep their custom user-facing error handling. Migrated 6 call sites: `conversion/document.rs` (LibreOffice), `telegram/commands/circle.rs` ×3 (ffmpeg video/retry/audio), `telegram/cuts.rs` (ffmpeg speed change), `telegram/voice_effects.rs` (ffmpeg voice effect). Download-path yt-dlp sites intentionally deferred — per CLAUDE.md they require a Railway smoke test before touching

### Changed
- **Refactor: reduce boilerplate across regex / error / ffmpeg / admin auth** (v0.36.2):
  - **`lazy-regex`** — 5 `Lazy<Regex>` definitions in `core/utils.rs` migrated to `lazy_regex!` macro. Regex patterns are now validated at compile time; a malformed pattern fails the build instead of panicking at first use
  - **`build_atempo_filter()` helper** in `telegram/commands/circle.rs` — the 5-line `if spd > 2.0 / else if spd < 0.5 / else` ffmpeg atempo chain was inlined verbatim 4 times; collapsed into a single function call (the ringtone branch uses `speed.map(build_atempo_filter).unwrap_or_else(...)`)
  - **`IgResultExt` trait + `ig_err()` helper** in `download/source/instagram.rs` — ~15 `.map_err(\|e\| AppError::Download(DownloadError::Instagram(format!("...: {}", e))))?` call sites now read `.ig_ctx("...")?`, and `Err(AppError::Download(DownloadError::Instagram("Rate limited".to_string())))` becomes `Err(ig_err("Rate limited"))`. Local to the Instagram module only
  - **`RequireAdmin` / `RequireAdminPost` axum extractors** in `core/web/auth.rs` — replaces the repeated `if let Err(resp) = verify_admin(&header_map, &state) { return resp; }` prologue at the top of ~20 admin handlers across `dashboard.rs`, `admin_queue.rs`, `admin_users.rs`, `admin_errors.rs`, `admin_misc.rs`. Auth is now enforced at the extractor layer — handlers that need admin access take a `RequireAdmin` / `RequireAdminPost` parameter, and the compiler refuses to build routes that forget it
  - Net: ~190 LOC deleted across 8 files, zero behavior change, all 560 workspace tests pass

### Fixed
- **SoundCloud track with `?in=...sets/...` query parameter misclassified as playlist** (v0.36.1) — `is_playlist_url` used substring matching against the entire URL string, so any SoundCloud track URL navigated to from inside a playlist (SoundCloud appends `?in=user/sets/foo`) triggered the `/sets/` check and was routed through `extract_latest_from_channel`, which returned a raw m3u8 CDN link that failed the source allowlist with "This website is not supported". Now all host/path checks in `is_playlist_url` operate on `url.host_str()` / `url.path()` separately — query parameters can no longer trigger false positives. Same fix also protects YouTube `/playlist`, `/@`, `/c/`, `/user/`, `/channel/` and Spotify `/playlist/`, `/album/` checks. Added two regression tests
- Updated all workspace dependencies to latest compatible versions (`cargo update`): tokio 1.50→1.51, reqwest stack, wasm-bindgen, uuid, sqlx transient deps, etc.

### Added
- **GIF creation from video** (v0.36.0) — after downloading any MP4 (or from a clip), press **🎞 GIF** to select a time range (max 30s) and get an animated GIF. Two-pass ffmpeg palette optimization for best quality. Works from both `/downloads` and `/cuts`. Segments over 30s are auto-truncated

- **Inline speed modifier** (v0.35.0) — append `2x`, `1.5x`, or `speed2` after a time range when sending a URL: `URL 2:48:45-2:49:59 2x` → bot downloads the clip and applies speed via ffmpeg `setpts`/`atempo`. Works for both MP4 and MP3 downloads. Speed stored in `preview_contexts` table alongside time range

### Changed
- **Tech debt**: Split `shared.rs` (6,920 lines) into `storage/shared/` module with 18 sub-files by domain (task_queue, users, subscriptions, analytics, etc.) — no functional changes
- **Tech debt**: Split `web_server.rs` (3,923 lines) into `core/web/` module with 9 sub-files (public, auth, dashboard, admin_users, admin_queue, admin_errors, admin_misc, types, helpers) — no functional changes
- **Tech debt**: Split `callbacks.rs` (2,176 lines) into 6 sub-modules in `downloads/` (send, clipping, speed, voice_lyrics, categories, cb_helpers) with `CallbackCtx` struct — no functional changes
- **Tech debt**: Split `bot_api_logger.rs` (1,677 lines) via `include!()` — macro-generated `@method` arms moved to `bot_api_logger_methods.rs` — no functional changes

### Added
- **Download pipeline optimizations for experimental mode** (v0.34.2–0.34.3):
  - Skip redundant yt-dlp metadata call (~6s) by reading title/artist from preview cache
  - Increase concurrent fragments from 8 → 16 for faster segmented downloads
  - Use `hqdefault.jpg` thumbnail instead of `maxresdefault.jpg` to skip compress step (~0.6s)
  - Preview format buttons now show estimated file sizes for all qualities (bitrate × duration fallback)
  - Skip ~6.5s livestream check by reading `is_live` from cached info JSON instead of yt-dlp network call
  - Fix "Unknown" size for 720p/1080p in preview buttons: estimate from `tbr × duration` when yt-dlp omits `filesize`/`filesize_approx` for adaptive DASH streams
- **Search by name** (v0.34.0) — type any song name (3+ chars) in chat → bot searches YouTube → shows results with download buttons. No URL needed. Rate-limited same as downloads
- **URL canonicalization** (v0.34.1) — normalizes URL variants for aggressive file_id cache. `youtu.be/ID`, `m.youtube.com/shorts/ID`, `music.youtube.com/watch?v=ID&si=...` all share the same cache entry. Covers 12 platforms: YouTube, Instagram, TikTok, Twitter/X, Spotify, SoundCloud, Vimeo, VK, Reddit, Facebook, Twitch, Bandcamp. Strips universal tracking params (utm_*, fbclid, gclid, si, etc.)
- Search results now respect user's format preference (mp3/mp4) from settings instead of hardcoded mp3
- Search status messages localized in all 4 languages (en, ru, fr, de)
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

## [0.33.1] - 2026-03-30

### Fixed
- **WITH_COOKIES fallback missing cache write** — `--load-info-json` optimization now works even when first proxy attempt fails (common on Railway)

### Changed
- Extract `pot_for_experimental()` helper — eliminates 8× duplicated POT logic across download tiers
- Extract `youtube_info_cache_path()` to `core::share` — single source of truth for cache path across crate boundary
- Remove redundant comments that paraphrase code

## [0.33.0] - 2026-03-23

### Added
- **Audio track language selection** for video downloads — YouTube videos with multiple audio tracks (original + dubbed) now show a `🔊 Audio track` button in the preview keyboard. Users can pick which language track to download (e.g., Japanese original vs English dub). Selection is stored per-URL and passed to yt-dlp via `[language=XX]` format filter with automatic fallback to best audio.

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
