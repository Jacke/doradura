# Explore Hub + Recent Timeline — Design Spec

**Date:** 2026-06-11
**Status:** Approved (design), pending spec review
**Sub-project:** A (of a 3-part decomposition — see "Context")

## Context

The user wants the Doradura bot to grow a **discovery hub ("Explore")**. During
brainstorming the scope decomposed into three independent sub-projects that share
one core:

- **A — Explore hub shell + "Recent" timeline** ← *this spec*. The hub's home tab:
  a convenient, date-bucketed timeline of the user's own past downloads with
  instant re-send.
- **B — Playlist Sync** (deferred). Unified `PlaylistProvider` trait + registry +
  sync engine, Spotify-first, subscription + proactive "new tracks → download?"
  ping. Plugs into the hub as a "Подписки" tab.
- **C — Discovery tabs** (deferred). Trending (from `popular_files`), personal
  recommendations (from `download_history`), platform Featured (`PlaylistDiscovery`
  trait), other users' public playlists. Each an incremental hub tab.

The **hybrid UI decision** governs A's architecture: build the timeline data layer
**once** as a backend service; render it via an **inline-keyboard UI now**, and
serve it as **JSON to a future Telegram Mini App** (separate spec). This spec
delivers the service + inline UI + a stub JSON route.

### Reused existing infrastructure (do NOT rebuild)

- `download_history` table + `SharedStorage` accessors: `get_download_history`,
  `get_download_history_filtered(user_id, &HistorySearch, limit, offset, date_from)`,
  `get_download_history_entry`, `period_cutoff()` (added alpha.28). Rows expose
  `id, url, title, format, downloaded_at (text), file_id, author`.
- `DownloadHistoryEntry` struct (`storage/shared/download_history.rs`).
- Instant re-send from a cached Telegram `file_id`:
  `bot.send_audio/send_video(chat, InputFile::file_id(FileId(id)))` — pattern
  already in `telegram/downloads/send.rs` and inline recents (alpha.34).
- Callback routing: `telegram/menu/callback_kind.rs` (`CallbackKind` enum,
  `strum` serialize, `parse()`) + `callback_router.rs`.
- axum web server + router: `core/web/mod.rs` (`Router::new().route(...)`),
  handlers in `core/web/public.rs`.
- i18n: `locales/{en-US,ru,fr,de}/main.ftl` (all four must be updated together).

## Goals / Non-goals

**Goals**
- A single backend `TimelineService` that produces a paginated, date-bucketed view
  of a user's downloads — the one source of truth for inline and Mini App.
- An inline-keyboard Explore hub with a tab bar; the **Recent** tab is live.
- Instant re-send of any timeline entry from its cached `file_id` (no re-download),
  with a download-by-URL fallback when `file_id` is absent.
- Entry points: `/explore` command + a main-menu button.
- A stub JSON route returning a `TimelinePage` (shape-frozen for the Mini App spec).

**Non-goals (deferred)**
- Mini App frontend (separate spec).
- Trending / Recommendations / Подписки / Featured tabs (sub-projects B, C) —
  rendered as disabled "скоро" tabs only.
- Editing history, multi-select, sharing — not in A (may come later).

## Architecture

Three units with clear boundaries:

### 1. `TimelineService` — backend, platform-neutral data (doracore)

New module `crates/doracore/src/explore/timeline.rs` (and `explore/mod.rs`).

```rust
/// A paginated, date-bucketed view of one user's downloads. Pure data:
/// rendered by the inline UI today, serialized to JSON for the Mini App later.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TimelinePage {
    pub buckets: Vec<TimelineBucket>,
    pub page: u32,
    pub total_pages: u32,
    pub total_entries: u32,
}

/// One date group, e.g. "Today" or "This week".
#[derive(Debug, Clone, serde::Serialize)]
pub struct TimelineBucket {
    pub label: BucketLabel,
    pub entries: Vec<TimelineEntry>,
}

/// Coarse, locale-independent date bucket. The renderer maps it to a localized
/// header; keeping it an enum (not a pre-formatted string) lets the inline UI and
/// the Mini App localize independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum BucketLabel { Today, Yesterday, ThisWeek, ThisMonth, Earlier }

/// One downloaded item in render-ready, platform-neutral form.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TimelineEntry {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub media: MediaKind,
    /// `Some` → instant re-send via Telegram file_id. `None` → fall back to
    /// re-download by `url`.
    pub file_id: Option<String>,
    pub url: String,
    pub at: chrono::DateTime<chrono::Utc>,
}

/// Media kind, derived from the history `format` column. Drives the row emoji
/// and which `send_*` method re-send uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum MediaKind { Audio, Video, VideoNote, Gif, Other }
```

```rust
/// Page size for one timeline page (inline UI). The Mini App may request more.
pub const TIMELINE_PAGE_SIZE: u32 = 10;

/// Build page `page` (0-based) of `user_id`'s timeline.
///
/// Pulls one page of history via `get_download_history_filtered` (DESC by
/// `downloaded_at`), maps rows to `TimelineEntry`, and groups them into date
/// buckets relative to `now`. `now` is injected for deterministic tests.
pub async fn build_timeline_page(
    storage: &SharedStorage,
    user_id: i64,
    page: u32,
    now: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<TimelinePage>;
```

Two pure, separately-tested helpers (no I/O):

```rust
/// Assign a UTC instant to its bucket relative to `now` (user-local handling is
/// a later refinement; v1 buckets in UTC).
pub fn bucket_for(at: DateTime<Utc>, now: DateTime<Utc>) -> BucketLabel;

/// Map a history `format` string ("mp3", "mp4", "video_note", "gif", …) to a
/// `MediaKind`.
pub fn media_kind_from_format(format: &str) -> MediaKind;
```

**Data flow:** `get_download_history_filtered(user_id, None, None, None, None)`
returns **all** of the user's rows DESC by `downloaded_at` (the accessor takes no
limit/offset). `build_timeline_page` then: maps each `DownloadHistoryEntry` to
`TimelineEntry` (parse `downloaded_at` text → `DateTime`,
`media_kind_from_format(format)`); computes `total_pages = ceil(len /
TIMELINE_PAGE_SIZE)`; slices the requested page; folds that page's entries into
date buckets (input already DESC, so bucket order is natural). v1 paginates
**in memory** — no new DB accessor. (Histories are modest; a paginated SQL
accessor is a later refinement if power-user histories grow large.)

### 2. Inline Explore hub — renderer (dorabot)

New module `crates/dorabot/src/telegram/explore/mod.rs` (+ `render.rs`).

- **Tab bar:** `[📜 Recent] [🔥 Trending] [⭐ Подписки]`. Recent active; the others
  are present but answer with a "🔜 скоро" toast (callback acknowledged, no nav) so
  the hub shape is visible from day one and B/C slot in without UI churn.
- **Message body:** localized, MarkdownV2-escaped timeline text — bucket headers
  (`🗓 Сегодня`) followed by numbered rows
  `N. {emoji} {Artist} — {Title} · {fmt} · {HH:MM}`.
- **Keyboard:** numbered re-send buttons (one per visible entry) + pagination
  `[‹] стр X/Y [›]` + the tab row.
- **Empty state:** friendly "ещё ничего не скачано — пришли ссылку" with a hint.

Mockup:

```
🎵 Doradura · Explore

[📜 Recent]·[🔥 Trending]·[⭐ Подписки]

🗓 Сегодня
 1. 🎵 Дора — Дорадура · mp3 · 14:22
 2. 🎬 Tame Impala — Let It Happen · mp4
🗓 Вчера
 3. 🎵 Eminem — Lose Yourself · mp3 · 21:10

[1️⃣][2️⃣][3️⃣]
[‹]  стр 1/4  [›]
```

### 3. Re-send action (dorabot)

Tapping entry `N` re-sends from the cached `file_id`:

- `file_id = Some` → `bot.send_audio/send_video/…` per `MediaKind` with
  `InputFile::file_id(FileId(id))` — instant, no download (existing send.rs pattern).
- `file_id = None` → fall back to the normal URL download pipeline for `entry.url`.
- On a stale-`file_id` Telegram error (file expired) → same URL fallback.

### 4. JSON route (stub for Mini App) (doracore)

Add to `core/web/mod.rs`:
`GET /api/timeline?user_id=&page=` → `Json<TimelinePage>` via `build_timeline_page`.
v1 returns the data shape only (auth via Telegram `initData` is specified in the
Mini App spec). Marked internal; rate/access hardening deferred with the frontend.

## Callbacks

Add `CallbackKind::Explore` (`strum` serialize `"exp"`), routed in
`callback_router.rs` to the explore handler. Wire format (compact, fits the 64-byte
Telegram callback budget):

- `exp:tab:{recent|trending|subs}` — switch tab (non-recent → "скоро" toast).
- `exp:page:{recent}:{page}` — paginate the Recent tab.
- `exp:rs:{history_id}` — re-send entry by its `download_history.id`.

The handler re-fetches the entry by id (`get_download_history_entry`) at action
time rather than trusting callback-encoded state.

## Entry points

- New `/explore` command (register in the command handler + bot command list).
- Main-menu button "📜 Мои треки" → emits `exp:tab:recent`.

## Persistence

**No new tables, no migration.** A reads existing `download_history`. (Sub-project
B adds tables; not here.)

## i18n

New fluent keys in all four locales (`en-US, ru, fr, de`):
`explore_title`, `explore_tab_recent`, `explore_tab_trending`, `explore_tab_subs`,
`explore_soon` ("🔜 скоро"), `explore_empty`, `explore_bucket_today`,
`explore_bucket_yesterday`, `explore_bucket_week`, `explore_bucket_month`,
`explore_bucket_earlier`, `explore_page` ("стр {$page}/{$total}"),
`explore_resent` ("📤 Отправил снова").

## Module layout

```
crates/doracore/src/explore/mod.rs          (new — pub mod timeline)
crates/doracore/src/explore/timeline.rs     (new — TimelineService + types + pure helpers + tests)
crates/doracore/src/lib.rs                   (add `pub mod explore;`)
crates/doracore/src/core/web/mod.rs          (add GET /api/timeline)
crates/doracore/src/core/web/public.rs       (add timeline_api_handler)
crates/dorabot/src/telegram/explore/mod.rs   (new — hub handler + callbacks)
crates/dorabot/src/telegram/explore/render.rs(new — text + keyboard builders + tests)
crates/dorabot/src/telegram/menu/callback_kind.rs   (add Explore variant)
crates/dorabot/src/telegram/menu/callback_router.rs (route exp:*)
crates/dorabot/src/telegram/handlers/commands.rs    (/explore command)
crates/dorabot/src/telegram/menu/...                (main-menu button)
locales/{en-US,ru,fr,de}/main.ftl                   (new keys)
crates/dorabot/Cargo.toml + CHANGELOG.md            (MINOR bump — new feature)
```

Boundary check: `TimelineService` knows nothing about Telegram (pure data, doracore);
the renderer knows nothing about SQL (consumes `TimelinePage`); re-send reuses the
existing send path. Each unit is understandable and testable in isolation.

## Error handling

- History fetch failure → log + user-facing "не удалось загрузить" message; never panic.
- Empty history → explicit empty state (not an error).
- Out-of-range page → clamp to `[0, total_pages)`.
- Re-send: stale/expired `file_id` → URL re-download fallback; total failure → the
  existing download error path (incl. the age-restricted classification from beta.2).
- `downloaded_at` parse failure on a row → skip the row, log a warning (don't fail
  the whole page).

## Testing

Pure unit tests (no Telegram/DB I/O):
- `bucket_for`: boundary cases around `now` (Today vs Yesterday at midnight,
  week/month edges) with injected `now`.
- `media_kind_from_format`: "mp3"→Audio, "mp4"→Video, "video_note"→VideoNote,
  "gif"→Gif, unknown→Other.
- `build_timeline_page` grouping: a fixture `Vec<DownloadHistoryEntry>` (via a thin
  seam over the accessor, or by testing the pure fold helper directly) yields the
  expected bucket order and pagination meta.
- `render`: deterministic text + keyboard for a fixture `TimelinePage`
  (MarkdownV2 escaping, numbered rows, pagination labels, empty state).

Compile/lint gate: `cargo check --workspace` + `cargo clippy --workspace
--all-targets -- -D warnings`. No yt-dlp arg changes → CLAUDE.md smoke-test gate
does not apply (re-send uses file_id / the existing pipeline unchanged).

## Versioning

MINOR bump (new feature) per CLAUDE.md SemVer rules: `0.51.0-beta.2` → next beta
that introduces the feature. CHANGELOG `[Unreleased]` entry under "Added".

## Open refinements (not blockers)

- User-local time zone for bucketing (v1 = UTC). Revisit if users report off-by-one
  day grouping.
- Paginated SQL accessor: v1 loads all of a user's history rows and paginates in
  memory. If power-user histories grow large, add a `LIMIT/OFFSET` accessor.
