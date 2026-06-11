# Explore Hub + Recent Timeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an inline "Explore" hub whose live "Recent" tab shows a user's downloads as a date-bucketed timeline with instant file_id re-send, backed by a reusable timeline service (also exposed as a JSON stub for a future Mini App).

**Architecture:** A pure `TimelineService` in `doracore` turns `download_history` rows into a paginated, date-bucketed `TimelinePage` (no Telegram, no SQL inside the pure helpers). `dorabot` renders that page as inline text + keyboard and handles tab/page/resend callbacks. A `GET /api/timeline` axum route serializes the same `TimelinePage`.

**Tech Stack:** Rust (edition 2024), tokio, serde, chrono, teloxide, axum, sqlx/rusqlite (dual backend via `SharedStorage`), Fluent i18n.

---

## ⚠️ Commit policy (project override)

CLAUDE.md forbids `git commit`/`push` without the user's explicit approval flag.
The hook `.claude/hooks/require-commit-approval.sh` blocks commits unless a fresh
`.claude/commit-approved` exists (10-min TTL), created by the user via
`!touch .claude/commit-approved`. **Before every `git commit` step below, ask the
user "Можно закоммитить?" and wait for the flag.** Commit steps are kept per the
TDD workflow but are gated on that approval.

## Build/verify commands (this repo)

`cargo` is reached via the stable toolchain bin:

```bash
export RUSTUP_HOME="$HOME/.rustup"
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
```

- Check: `cargo check -p doracore -p doradura`
- Test (core): `cargo test -p doracore explore`
- Test (bot): `cargo test -p doradura explore`
- Lint: `cargo clippy -p doracore -p doradura --all-targets -- -D warnings`

No yt-dlp arg changes anywhere in this plan → the CLAUDE.md yt-dlp smoke-test gate does NOT apply.

## File structure

| File | Responsibility |
|------|----------------|
| `crates/doracore/src/explore/mod.rs` (new) | `pub mod timeline;` |
| `crates/doracore/src/explore/timeline.rs` (new) | Types + pure helpers + `build_timeline_page` + tests |
| `crates/doracore/src/lib.rs` (modify) | `pub mod explore;` |
| `crates/doracore/src/core/web/mod.rs` (modify) | route `GET /api/timeline` |
| `crates/doracore/src/core/web/public.rs` (modify) | `timeline_api_handler` |
| `crates/dorabot/src/telegram/explore/mod.rs` (new) | hub callback handler (tab/page/resend) |
| `crates/dorabot/src/telegram/explore/render.rs` (new) | text + keyboard builders + tests |
| `crates/dorabot/src/telegram/mod.rs` (modify) | `pub mod explore;` (or wherever submodules register) |
| `crates/dorabot/src/telegram/menu/callback_kind.rs` (modify) | `Explore` variant |
| `crates/dorabot/src/telegram/menu/callback_router.rs` (modify) | dispatch `exp:*` |
| `crates/dorabot/src/telegram/handlers/commands.rs` (modify) | `/explore` command |
| `locales/{en-US,ru,fr,de}/main.ftl` (modify) | new `explore_*` keys |
| `crates/dorabot/Cargo.toml` + `CHANGELOG.md` (modify) | MINOR bump + entry |

---

## Task 1: Core types + `media_kind_from_format`

**Files:**
- Create: `crates/doracore/src/explore/mod.rs`
- Create: `crates/doracore/src/explore/timeline.rs`
- Modify: `crates/doracore/src/lib.rs` (add `pub mod explore;` near the other `pub mod` lines)

- [ ] **Step 1: Create `explore/mod.rs`**

```rust
//! Explore hub backend: discovery + timeline data, rendered by the bot UI and
//! (later) a Telegram Mini App. Platform-neutral, no Telegram types here.

pub mod timeline;
```

- [ ] **Step 2: Write `explore/timeline.rs` types + the failing test for `media_kind_from_format`**

```rust
//! Timeline service: turns `download_history` rows into a paginated,
//! date-bucketed view. Pure helpers (`bucket_for`, `media_kind_from_format`,
//! `group_into_buckets`) carry no I/O and are unit-tested directly.

use chrono::{DateTime, Datelike, Utc};
use serde::Serialize;

/// Page size for one inline timeline page. The Mini App may request more.
pub const TIMELINE_PAGE_SIZE: usize = 10;

/// Media kind, derived from the history `format` column. Drives the row emoji
/// and which `send_*` method re-send uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MediaKind { Audio, Video, VideoNote, Gif, Other }

/// Coarse, locale-independent date bucket. The renderer maps it to a localized
/// header so the inline UI and the Mini App localize independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum BucketLabel { Today, Yesterday, ThisWeek, ThisMonth, Earlier }

/// One downloaded item in render-ready, platform-neutral form.
#[derive(Debug, Clone, Serialize)]
pub struct TimelineEntry {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub media: MediaKind,
    /// `Some` → instant re-send via Telegram file_id. `None` → re-download `url`.
    pub file_id: Option<String>,
    pub url: String,
    pub at: DateTime<Utc>,
}

/// One date group, e.g. "Today".
#[derive(Debug, Clone, Serialize)]
pub struct TimelineBucket {
    pub label: BucketLabel,
    pub entries: Vec<TimelineEntry>,
}

/// A paginated, date-bucketed view of one user's downloads.
#[derive(Debug, Clone, Serialize)]
pub struct TimelinePage {
    pub buckets: Vec<TimelineBucket>,
    pub page: u32,
    pub total_pages: u32,
    pub total_entries: u32,
}

/// Map a history `format` string to a `MediaKind`.
pub fn media_kind_from_format(format: &str) -> MediaKind {
    match format.trim().to_lowercase().as_str() {
        "mp3" | "m4a" | "m4r" | "opus" | "audio" => MediaKind::Audio,
        "mp4" | "mkv" | "webm" | "video" => MediaKind::Video,
        "video_note" | "circle" | "note" => MediaKind::VideoNote,
        "gif" => MediaKind::Gif,
        _ => MediaKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_kind_maps_known_formats() {
        assert_eq!(media_kind_from_format("mp3"), MediaKind::Audio);
        assert_eq!(media_kind_from_format("MP4"), MediaKind::Video);
        assert_eq!(media_kind_from_format("video_note"), MediaKind::VideoNote);
        assert_eq!(media_kind_from_format("gif"), MediaKind::Gif);
        assert_eq!(media_kind_from_format("srt"), MediaKind::Other);
    }
}
```

- [ ] **Step 3: Add `pub mod explore;` to `crates/doracore/src/lib.rs`** (next to other top-level `pub mod` declarations).

- [ ] **Step 4: Run the test — expect PASS**

```bash
cargo test -p doracore explore::timeline::tests::media_kind_maps_known_formats
```
Expected: PASS.

- [ ] **Step 5: Lint**

```bash
cargo clippy -p doracore --all-targets -- -D warnings
```
Expected: clean.

- [ ] **Step 6: Commit** (gated on approval flag — see Commit policy)

```bash
git add crates/doracore/src/explore crates/doracore/src/lib.rs
git commit -m "feat(explore): timeline core types + media_kind_from_format"
```

---

## Task 2: `bucket_for` date bucketing

**Files:**
- Modify: `crates/doracore/src/explore/timeline.rs`

- [ ] **Step 1: Add the failing test** (append to the `tests` mod)

```rust
#[test]
fn bucket_for_classifies_relative_to_now() {
    use chrono::TimeZone;
    let now = Utc.with_ymd_and_hms(2026, 6, 11, 12, 0, 0).unwrap();
    let today = Utc.with_ymd_and_hms(2026, 6, 11, 1, 0, 0).unwrap();
    let yesterday = Utc.with_ymd_and_hms(2026, 6, 10, 23, 0, 0).unwrap();
    let three_days = Utc.with_ymd_and_hms(2026, 6, 8, 9, 0, 0).unwrap();
    let twenty_days = Utc.with_ymd_and_hms(2026, 5, 25, 9, 0, 0).unwrap();
    let old = Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap();

    assert_eq!(bucket_for(today, now), BucketLabel::Today);
    assert_eq!(bucket_for(yesterday, now), BucketLabel::Yesterday);
    assert_eq!(bucket_for(three_days, now), BucketLabel::ThisWeek);
    assert_eq!(bucket_for(twenty_days, now), BucketLabel::ThisMonth);
    assert_eq!(bucket_for(old, now), BucketLabel::Earlier);
}
```

- [ ] **Step 2: Run — expect FAIL** (`bucket_for` undefined)

```bash
cargo test -p doracore explore::timeline::tests::bucket_for_classifies_relative_to_now
```
Expected: FAIL (cannot find function `bucket_for`).

- [ ] **Step 3: Implement `bucket_for`** (add above the `tests` mod)

```rust
/// Assign a UTC instant to its bucket relative to `now`. Buckets compare on the
/// calendar day (UTC): same day = Today, day-1 = Yesterday, within 7 days =
/// ThisWeek, within 31 days = ThisMonth, else Earlier.
pub fn bucket_for(at: DateTime<Utc>, now: DateTime<Utc>) -> BucketLabel {
    let days = (now.date_naive() - at.date_naive()).num_days();
    match days {
        d if d <= 0 => BucketLabel::Today,
        1 => BucketLabel::Yesterday,
        2..=6 => BucketLabel::ThisWeek,
        7..=30 => BucketLabel::ThisMonth,
        _ => BucketLabel::Earlier,
    }
}
```

- [ ] **Step 4: Run — expect PASS**

```bash
cargo test -p doracore explore::timeline::tests::bucket_for_classifies_relative_to_now
```
Expected: PASS.

- [ ] **Step 5: Commit** (gated)

```bash
git add crates/doracore/src/explore/timeline.rs
git commit -m "feat(explore): date bucketing (bucket_for)"
```

---

## Task 3: `group_into_buckets` fold

**Files:**
- Modify: `crates/doracore/src/explore/timeline.rs`

- [ ] **Step 1: Add the failing test**

```rust
#[test]
fn group_into_buckets_preserves_desc_order_and_groups() {
    use chrono::TimeZone;
    let now = Utc.with_ymd_and_hms(2026, 6, 11, 12, 0, 0).unwrap();
    let mk = |id: i64, at: DateTime<Utc>| TimelineEntry {
        id, title: format!("t{id}"), artist: "a".into(), media: MediaKind::Audio,
        file_id: None, url: "u".into(), at,
    };
    let entries = vec![
        mk(1, Utc.with_ymd_and_hms(2026, 6, 11, 9, 0, 0).unwrap()), // Today
        mk(2, Utc.with_ymd_and_hms(2026, 6, 11, 8, 0, 0).unwrap()), // Today
        mk(3, Utc.with_ymd_and_hms(2026, 6, 10, 8, 0, 0).unwrap()), // Yesterday
    ];
    let buckets = group_into_buckets(&entries, now);
    assert_eq!(buckets.len(), 2);
    assert_eq!(buckets[0].label, BucketLabel::Today);
    assert_eq!(buckets[0].entries.len(), 2);
    assert_eq!(buckets[1].label, BucketLabel::Yesterday);
    assert_eq!(buckets[1].entries.len(), 1);
}
```

- [ ] **Step 2: Run — expect FAIL**

```bash
cargo test -p doracore explore::timeline::tests::group_into_buckets_preserves_desc_order_and_groups
```
Expected: FAIL (cannot find function `group_into_buckets`).

- [ ] **Step 3: Implement** (add above `tests`)

```rust
/// Fold DESC-ordered entries into consecutive date buckets. Assumes `entries`
/// is already sorted newest-first (as the history accessor returns it), so a
/// single pass yields buckets in display order without sorting.
pub fn group_into_buckets(entries: &[TimelineEntry], now: DateTime<Utc>) -> Vec<TimelineBucket> {
    let mut buckets: Vec<TimelineBucket> = Vec::new();
    for entry in entries {
        let label = bucket_for(entry.at, now);
        match buckets.last_mut() {
            Some(b) if b.label == label => b.entries.push(entry.clone()),
            _ => buckets.push(TimelineBucket { label, entries: vec![entry.clone()] }),
        }
    }
    buckets
}
```

- [ ] **Step 4: Run — expect PASS**

```bash
cargo test -p doracore explore::timeline::tests::group_into_buckets_preserves_desc_order_and_groups
```
Expected: PASS.

- [ ] **Step 5: Commit** (gated)

```bash
git add crates/doracore/src/explore/timeline.rs
git commit -m "feat(explore): group_into_buckets fold"
```

---

## Task 4: `build_timeline_page` (I/O entry point)

**Files:**
- Modify: `crates/doracore/src/explore/timeline.rs`
- Reference: `crates/doracore/src/storage/shared/download_history.rs` (`get_download_history_filtered(user_id, file_type, search_text, category, date_from) -> Result<Vec<DownloadHistoryEntry>>`, returns ALL rows DESC; `DownloadHistoryEntry { id, url, title, format, downloaded_at: String, file_id: Option<String>, author: Option<String> }`)

- [ ] **Step 1: Add a pure paginator test** (the I/O is thin; test the pure slice+meta helper)

```rust
#[test]
fn paginate_computes_meta_and_slices() {
    use chrono::TimeZone;
    let now = Utc.with_ymd_and_hms(2026, 6, 11, 12, 0, 0).unwrap();
    let all: Vec<TimelineEntry> = (0..23).map(|i| TimelineEntry {
        id: i, title: format!("t{i}"), artist: "a".into(), media: MediaKind::Audio,
        file_id: None, url: "u".into(),
        at: Utc.with_ymd_and_hms(2026, 6, 11, 9, 0, 0).unwrap(),
    }).collect();

    let p0 = paginate(all.clone(), 0, now);
    assert_eq!(p0.total_entries, 23);
    assert_eq!(p0.total_pages, 3);          // ceil(23/10)
    assert_eq!(p0.page, 0);
    assert_eq!(p0.buckets.iter().map(|b| b.entries.len()).sum::<usize>(), 10);

    let p2 = paginate(all.clone(), 2, now);
    assert_eq!(p2.page, 2);
    assert_eq!(p2.buckets.iter().map(|b| b.entries.len()).sum::<usize>(), 3); // last page

    let clamped = paginate(all, 99, now);   // out of range clamps to last
    assert_eq!(clamped.page, 2);
}
```

- [ ] **Step 2: Run — expect FAIL** (`paginate` undefined)

```bash
cargo test -p doracore explore::timeline::tests::paginate_computes_meta_and_slices
```
Expected: FAIL.

- [ ] **Step 3: Implement `paginate` + `build_timeline_page`**

```rust
use crate::storage::shared::SharedStorage;

/// Slice `all` (DESC) into page `page`, clamping out-of-range pages to the last
/// page, and bucket that page. Pure — `build_timeline_page` feeds it DB rows.
pub fn paginate(all: Vec<TimelineEntry>, page: u32, now: DateTime<Utc>) -> TimelinePage {
    let total_entries = all.len() as u32;
    let total_pages = ((all.len() + TIMELINE_PAGE_SIZE - 1) / TIMELINE_PAGE_SIZE).max(1) as u32;
    let page = page.min(total_pages - 1);
    let start = (page as usize) * TIMELINE_PAGE_SIZE;
    let slice: Vec<TimelineEntry> = all.into_iter().skip(start).take(TIMELINE_PAGE_SIZE).collect();
    TimelinePage { buckets: group_into_buckets(&slice, now), page, total_pages, total_entries }
}

/// Build page `page` (0-based) of `user_id`'s download timeline. `now` is
/// injected for deterministic tests.
pub async fn build_timeline_page(
    storage: &SharedStorage,
    user_id: i64,
    page: u32,
    now: DateTime<Utc>,
) -> anyhow::Result<TimelinePage> {
    let rows = storage
        .get_download_history_filtered(user_id, None, None, None, None)
        .await?;
    let entries: Vec<TimelineEntry> = rows
        .into_iter()
        .filter_map(|r| {
            // history stores `downloaded_at` as text; skip unparseable rows.
            let at = DateTime::parse_from_rfc3339(&r.downloaded_at)
                .map(|dt| dt.with_timezone(&Utc))
                .ok()?;
            Some(TimelineEntry {
                id: r.id,
                title: r.title,
                artist: r.author.unwrap_or_default(),
                media: media_kind_from_format(&r.format),
                file_id: r.file_id,
                url: r.url,
                at,
            })
        })
        .collect();
    Ok(paginate(entries, page, now))
}
```

> NOTE on timestamp parsing: `downloaded_at` text format may not be RFC3339 (e.g.
> `"2026-06-11 09:00:00"`). During implementation, print one row's
> `downloaded_at` (a quick `dbg!` in a throwaway test or check the SQL
> `downloaded_at::text` output) and, if needed, add a
> `NaiveDateTime::parse_from_str(&r.downloaded_at, "%Y-%m-%d %H:%M:%S%.f")`
> fallback before the rfc3339 attempt. Keep the "skip unparseable row" behavior.

- [ ] **Step 4: Run — expect PASS**

```bash
cargo test -p doracore explore::timeline
```
Expected: all timeline tests PASS.

- [ ] **Step 5: Lint + check**

```bash
cargo clippy -p doracore --all-targets -- -D warnings
```
Expected: clean.

- [ ] **Step 6: Commit** (gated)

```bash
git add crates/doracore/src/explore/timeline.rs
git commit -m "feat(explore): build_timeline_page + in-memory pagination"
```

---

## Task 5: JSON stub route `GET /api/timeline`

**Files:**
- Modify: `crates/doracore/src/core/web/public.rs` (add handler — mirror the existing `share_api_handler` for state extraction / `Json` response)
- Modify: `crates/doracore/src/core/web/mod.rs` (add route after `/api/s/{id}`)

- [ ] **Step 1: Read the existing handler pattern**

```bash
grep -n "share_api_handler" crates/doracore/src/core/web/public.rs
```
Read it to see how `State`/storage and `Json` are used.

- [ ] **Step 2: Add `timeline_api_handler` to `public.rs`**

```rust
/// `GET /api/timeline?user_id=<i64>&page=<u32>` → JSON `TimelinePage`.
/// Internal/stub for the future Mini App; auth (Telegram initData) is added
/// with the frontend spec. Mirror `share_api_handler`'s `State` extraction.
pub async fn timeline_api_handler(
    axum::extract::State(state): axum::extract::State<WebState>, // use the same State type as share_api_handler
    axum::extract::Query(q): axum::extract::Query<TimelineQuery>,
) -> impl axum::response::IntoResponse {
    let now = chrono::Utc::now();
    match crate::explore::timeline::build_timeline_page(&state.storage, q.user_id, q.page.unwrap_or(0), now).await {
        Ok(page) => axum::Json(page).into_response(),
        Err(e) => {
            log::warn!("timeline_api_handler: {e}");
            (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "error").into_response()
        }
    }
}

#[derive(serde::Deserialize)]
pub struct TimelineQuery { pub user_id: i64, pub page: Option<u32> }
```

> Adapt `WebState`/`state.storage` to the actual field names used by
> `share_api_handler` (read them in Step 1; do not invent names).

- [ ] **Step 3: Register the route in `mod.rs`** (after the `/api/s/{id}` line)

```rust
        .route("/api/timeline", get(public::timeline_api_handler))
```

- [ ] **Step 4: Check**

```bash
cargo check -p doracore
```
Expected: compiles.

- [ ] **Step 5: Commit** (gated)

```bash
git add crates/doracore/src/core/web/public.rs crates/doracore/src/core/web/mod.rs
git commit -m "feat(explore): GET /api/timeline JSON stub for Mini App"
```

---

## Task 6: i18n keys (×4 locales)

**Files:**
- Modify: `locales/en-US/main.ftl`, `locales/ru/main.ftl`, `locales/fr/main.ftl`, `locales/de/main.ftl`

- [ ] **Step 1: Append keys to each locale** (translate values per locale; keys identical). English shown; mirror into ru/fr/de.

```ftl
explore_title = 🎵 Doradura · Explore
explore_tab_recent = 📜 Recent
explore_tab_trending = 🔥 Trending
explore_tab_subs = ⭐ Subscriptions
explore_soon = 🔜 Coming soon
explore_empty = Nothing downloaded yet — send a link to start.
explore_bucket_today = 🗓 Today
explore_bucket_yesterday = 🗓 Yesterday
explore_bucket_week = 🗓 This week
explore_bucket_month = 🗓 This month
explore_bucket_earlier = 🗓 Earlier
explore_page = page { $page }/{ $total }
explore_resent = 📤 Sent again
explore_load_failed = ❌ Couldn't load your timeline. Try again.
```

Russian values (`locales/ru/main.ftl`): `📜 Лента`, `🔥 Тренды`, `⭐ Подписки`, `🔜 Скоро`, `Пока ничего не скачано — пришли ссылку.`, `🗓 Сегодня`, `🗓 Вчера`, `🗓 Эта неделя`, `🗓 Этот месяц`, `🗓 Ранее`, `стр { $page }/{ $total }`, `📤 Отправил снова`, `❌ Не удалось загрузить ленту. Попробуй ещё раз.`. Provide natural fr/de equivalents.

- [ ] **Step 2: Check the bot still builds** (Fluent files are validated at runtime; ensure no syntax typo)

```bash
cargo check -p doradura
```
Expected: compiles.

- [ ] **Step 3: Commit** (gated)

```bash
git add locales/en-US/main.ftl locales/ru/main.ftl locales/fr/main.ftl locales/de/main.ftl
git commit -m "feat(explore): i18n keys for explore hub (en/ru/fr/de)"
```

---

## Task 7: Inline renderer (text + keyboard)

**Files:**
- Create: `crates/dorabot/src/telegram/explore/mod.rs`
- Create: `crates/dorabot/src/telegram/explore/render.rs`
- Modify: `crates/dorabot/src/telegram/mod.rs` (add `pub mod explore;` where sibling modules are declared)
- Reference: existing `crate::telegram::cb(label, data)` helper (returns an `InlineKeyboardButton`); existing MarkdownV2 escape helper (`grep -rn "fn escape_markdown" crates/`).

- [ ] **Step 1: Create `explore/mod.rs` shell**

```rust
//! Inline Explore hub: renders the timeline (Recent tab) and handles
//! tab/page/resend callbacks. Discovery tabs (Trending/Subscriptions) are
//! placeholders until sub-projects C/B land.

pub mod render;
```

- [ ] **Step 2: Write `render.rs` with a failing text test**

```rust
//! Pure builders: a `TimelinePage` → message text + inline keyboard. No I/O,
//! no Telegram API calls — fully unit-testable.

use doracore::explore::timeline::{BucketLabel, MediaKind, TimelinePage};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

fn media_emoji(m: MediaKind) -> &'static str {
    match m {
        MediaKind::Audio => "🎵",
        MediaKind::Video => "🎬",
        MediaKind::VideoNote => "⭕",
        MediaKind::Gif => "🎞",
        MediaKind::Other => "📄",
    }
}

/// Build the timeline message body (MarkdownV2). `bucket_header` maps a
/// `BucketLabel` to a localized header string (injected so this stays pure).
pub fn render_timeline_text(
    page: &TimelinePage,
    title: &str,
    empty_msg: &str,
    bucket_header: &dyn Fn(BucketLabel) -> String,
    esc: &dyn Fn(&str) -> String,
) -> String {
    if page.total_entries == 0 {
        return format!("{title}\n\n{}", esc(empty_msg));
    }
    let mut out = format!("{title}\n");
    let mut n = 0u32;
    for bucket in &page.buckets {
        out.push_str(&format!("\n{}\n", bucket_header(bucket.label)));
        for e in &bucket.entries {
            n += 1;
            let artist = if e.artist.trim().is_empty() { String::new() } else { format!("{} — ", esc(&e.artist)) };
            out.push_str(&format!(
                " {n}\\. {} {}{} · {}\n",
                media_emoji(e.media), artist, esc(&e.title), esc(&e.media_label())
            ));
        }
    }
    out
}

/// Build the inline keyboard: numbered resend buttons (by history id) + pager + tabs.
pub fn render_timeline_keyboard(
    page: &TimelinePage,
    tab_recent: &str, tab_trending: &str, tab_subs: &str,
    page_label: &str,
) -> InlineKeyboardMarkup {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    // Numbered resend row(s): one button per visible entry.
    let mut num_row: Vec<InlineKeyboardButton> = Vec::new();
    let mut n = 0u32;
    for bucket in &page.buckets {
        for e in &bucket.entries {
            n += 1;
            num_row.push(crate::telegram::cb(&format!("{n}"), format!("exp:rs:{}", e.id)));
            if num_row.len() == 5 { rows.push(std::mem::take(&mut num_row)); }
        }
    }
    if !num_row.is_empty() { rows.push(num_row); }

    // Pager: ‹  page X/Y  ›
    let mut pager: Vec<InlineKeyboardButton> = Vec::new();
    if page.page > 0 {
        pager.push(crate::telegram::cb("‹", format!("exp:page:recent:{}", page.page - 1)));
    }
    pager.push(crate::telegram::cb(page_label, "exp:noop".to_string()));
    if page.page + 1 < page.total_pages {
        pager.push(crate::telegram::cb("›", format!("exp:page:recent:{}", page.page + 1)));
    }
    rows.push(pager);

    // Tabs
    rows.push(vec![
        crate::telegram::cb(tab_recent, "exp:tab:recent".to_string()),
        crate::telegram::cb(tab_trending, "exp:tab:trending".to_string()),
        crate::telegram::cb(tab_subs, "exp:tab:subs".to_string()),
    ]);

    InlineKeyboardMarkup::new(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use doracore::explore::timeline::{paginate, TimelineEntry};

    fn entry(id: i64) -> TimelineEntry {
        TimelineEntry { id, title: "Song".into(), artist: "Art".into(),
            media: MediaKind::Audio, file_id: Some("F".into()), url: "u".into(),
            at: Utc.with_ymd_and_hms(2026, 6, 11, 9, 0, 0).unwrap() }
    }

    #[test]
    fn renders_numbered_rows_and_header() {
        let now = Utc.with_ymd_and_hms(2026, 6, 11, 12, 0, 0).unwrap();
        let page = paginate(vec![entry(1), entry(2)], 0, now);
        let text = render_timeline_text(
            &page, "TITLE", "EMPTY",
            &|_| "HEADER".to_string(),
            &|s| s.to_string(),
        );
        assert!(text.contains("HEADER"));
        assert!(text.contains(" 1\\. 🎵"));
        assert!(text.contains(" 2\\. 🎵"));
    }

    #[test]
    fn empty_page_shows_empty_message() {
        let now = Utc.with_ymd_and_hms(2026, 6, 11, 12, 0, 0).unwrap();
        let page = paginate(vec![], 0, now);
        let text = render_timeline_text(&page, "TITLE", "EMPTY", &|_| "H".to_string(), &|s| s.to_string());
        assert!(text.contains("EMPTY"));
    }
}
```

- [ ] **Step 3: Add `media_label()` helper to `MediaKind` in doracore** (`explore/timeline.rs`), used by the renderer:

```rust
impl MediaKind {
    /// Short label for captions, e.g. "mp3"/"mp4"/"note"/"gif".
    pub fn media_label(self) -> &'static str {
        match self {
            MediaKind::Audio => "mp3",
            MediaKind::Video => "mp4",
            MediaKind::VideoNote => "note",
            MediaKind::Gif => "gif",
            MediaKind::Other => "file",
        }
    }
}
```

And change the renderer call `e.media_label()` → `e.media.media_label()` (entries hold a `MediaKind`).

- [ ] **Step 4: Run — expect PASS**

```bash
cargo test -p doradura explore::render
cargo test -p doracore explore
```
Expected: PASS.

- [ ] **Step 5: Commit** (gated)

```bash
git add crates/doracore/src/explore/timeline.rs crates/dorabot/src/telegram/explore crates/dorabot/src/telegram/mod.rs
git commit -m "feat(explore): inline timeline renderer (text + keyboard)"
```

---

## Task 8: Callback kind + router + handler

**Files:**
- Modify: `crates/dorabot/src/telegram/menu/callback_kind.rs` (add variant)
- Modify: `crates/dorabot/src/telegram/menu/callback_router.rs` (dispatch)
- Modify: `crates/dorabot/src/telegram/explore/mod.rs` (handler)
- Reference: existing resend send pattern in `telegram/downloads/send.rs:376-388`
  (`bot.send_audio/send_video(chat, InputFile::file_id(FileId(id)))`); accessor
  `SharedStorage::get_download_history_entry(user_id, id)`.

- [ ] **Step 1: Add `Explore` variant to `CallbackKind`** (mirror the existing `#[strum(serialize = "downloads")]` style)

```rust
    #[strum(serialize = "exp")]
    Explore,
```

- [ ] **Step 2: Add the parse test** (mirror existing `parses_phase_c_prefixes` test)

```rust
    #[test]
    fn parses_explore_prefix() {
        assert_eq!(CallbackKind::parse("exp:tab:recent"), Some(CallbackKind::Explore));
        assert_eq!(CallbackKind::parse("exp:rs:42"), Some(CallbackKind::Explore));
    }
```

Run: `cargo test -p doradura parses_explore_prefix` → PASS.

- [ ] **Step 3: Route `exp:*` in `callback_router.rs`** to `explore::handle_explore_callback(bot, q, data, storage).await` (mirror how the `downloads`/`history` arms dispatch — read the existing match first).

- [ ] **Step 4: Implement the handler in `explore/mod.rs`**

```rust
use crate::storage::SharedStorage;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{CallbackQuery, FileId, InputFile, ParseMode};

/// Dispatch `exp:*` callbacks: tab switch, pagination, resend.
pub async fn handle_explore_callback(
    bot: crate::telegram::Bot,
    q: CallbackQuery,
    data: &str,
    storage: Arc<SharedStorage>,
) -> anyhow::Result<()> {
    let user_id = q.from.id.0 as i64;
    let parts: Vec<&str> = data.split(':').collect(); // ["exp", action, ...]
    match parts.as_slice() {
        ["exp", "tab", "recent"] => show_recent(&bot, &q, &storage, user_id, 0).await,
        ["exp", "page", "recent", p] => {
            let page = p.parse::<u32>().unwrap_or(0);
            show_recent(&bot, &q, &storage, user_id, page).await
        }
        ["exp", "tab", _other] => {
            // Trending / Subscriptions not built yet (sub-projects C / B).
            bot.answer_callback_query(q.id.clone())
                .text(t_explore_soon(&q)) // localized "🔜 Coming soon"
                .show_alert(false).await?;
            Ok(())
        }
        ["exp", "rs", id] => {
            let hist_id = id.parse::<i64>().unwrap_or(0);
            resend_entry(&bot, &q, &storage, user_id, hist_id).await
        }
        _ => { bot.answer_callback_query(q.id.clone()).await?; Ok(()) }
    }
}
```

`show_recent`: `build_timeline_page` → `render_timeline_text/keyboard` → edit the
callback's message (or send new if none) with `ParseMode::MarkdownV2`; on error,
answer with the localized `explore_load_failed`.

`resend_entry`: fetch via `storage.get_download_history_entry(user_id, hist_id)`.
If `file_id` present → send via the `MediaKind`-appropriate method
(`send_audio`/`send_video`/`send_video_note`/`send_animation`) using
`InputFile::file_id(FileId(id))` (mirror `send.rs:376-388`). On a Telegram
"file reference expired" error OR `file_id == None` → fall back to the normal URL
download path for `entry.url`. Always `answer_callback_query` (with `explore_resent`
on success).

> Localization: call the project's `t(...)`/`t_args(...)` i18n helper with the
> user's locale exactly as neighbouring handlers do — `grep -rn "fn t(" crates/dorabot/src`
> and copy the call shape. Use the `explore_*` keys from Task 6.

- [ ] **Step 5: Check + lint**

```bash
cargo check -p doradura && cargo clippy -p doradura --all-targets -- -D warnings
```
Expected: clean.

- [ ] **Step 6: Commit** (gated)

```bash
git add crates/dorabot/src/telegram/menu/callback_kind.rs crates/dorabot/src/telegram/menu/callback_router.rs crates/dorabot/src/telegram/explore/mod.rs
git commit -m "feat(explore): callback routing + tab/page/resend handler"
```

---

## Task 9: `/explore` command + menu button

**Files:**
- Modify: `crates/dorabot/src/telegram/handlers/commands.rs` (add `/explore`)
- Modify: the main-menu builder (find it: `grep -rn "fn.*main_menu\|services_text\|InlineKeyboardMarkup::new" crates/dorabot/src/telegram/menu/*.rs`)

- [ ] **Step 1: Add `/explore` handling** — on `/explore`, call
  `explore::show_recent(&bot, /* no callback */ …, &storage, user_id, 0)` by
  sending a fresh message. Provide a `show_recent_fresh(bot, chat_id, storage, user_id)`
  variant in `explore/mod.rs` that **sends** (not edits) the timeline, so both the
  command and the menu button reuse it. Mirror how another command (e.g. the
  existing `/downloads` / history command) sends its first message.

- [ ] **Step 2: Add a menu button "📜 Мои треки"** to the main menu keyboard with
  callback data `exp:tab:recent` (the existing router already dispatches it).

- [ ] **Step 3: Manual smoke (local build only)**

```bash
cargo check -p doradura
```
Expected: compiles. (Live Telegram verification happens after deploy.)

- [ ] **Step 4: Commit** (gated)

```bash
git add crates/dorabot/src/telegram/handlers/commands.rs crates/dorabot/src/telegram/menu
git commit -m "feat(explore): /explore command + main-menu entry"
```

---

## Task 10: Version bump, CHANGELOG, full verification

**Files:**
- Modify: `crates/dorabot/Cargo.toml` (version)
- Modify: `CHANGELOG.md` (`[Unreleased]` → Added)

- [ ] **Step 1: Bump version** — MINOR (new feature) per CLAUDE.md. If beta.2 is
  already committed, go to `0.51.0-beta.3`; otherwise coordinate with the pending
  beta.2. Set `crates/dorabot/Cargo.toml` `version`.

- [ ] **Step 2: CHANGELOG entry** under `## [Unreleased]` → `### Added`:

```markdown
- **Explore-хаб + вкладка «Лента» (Recent timeline)** (v0.51.0-beta.3) — новый inline-хаб `/explore` (+ кнопка меню «📜 Мои треки») показывает скачанное юзером в виде таймлайна по датам (Сегодня/Вчера/Эта неделя/Этот месяц/Ранее) с мгновенным resend из `file_id` (без перекачки; fallback на скачивание по URL если file_id протух/отсутствует). Backend `TimelineService` (`doracore::explore::timeline`) — единый источник данных: рендерится inline сейчас и отдаётся как JSON `GET /api/timeline` для будущего Mini App. Табы Trending/Подписки — заглушки «🔜 скоро» (под-проекты C/B). Без миграций (читаем `download_history`). i18n ×4. Пагинация в памяти (10/стр).
```

- [ ] **Step 3: Full workspace verification**

```bash
cargo check -p doracore -p doradura
cargo test -p doracore explore && cargo test -p doradura explore
cargo clippy -p doracore -p doradura --all-targets -- -D warnings
```
Expected: all green.

- [ ] **Step 4: Commit** (gated)

```bash
git add crates/dorabot/Cargo.toml CHANGELOG.md
git commit -m "chore(explore): version bump + CHANGELOG (v0.51.0-beta.3)"
```

---

## Self-review checklist (completed by plan author)

- **Spec coverage:** TimelineService (T1–4), JSON route (T5), inline hub + tabs +
  resend (T7–8), entry points (T9), i18n (T6), no migration (honored), version/
  CHANGELOG (T10). All spec sections mapped. ✅
- **Placeholders:** none — pure-logic tasks carry full code; UI-glue tasks reference
  exact existing functions/files to mirror (not vague "handle it"). The two
  intentional "read existing X then mirror" notes (i18n `t()` call shape, `WebState`
  field names) are grounded with the exact grep to run. ✅
- **Type consistency:** `TimelineEntry`/`TimelinePage`/`MediaKind`/`BucketLabel`,
  `build_timeline_page(storage,user_id,page,now)`, `paginate(all,page,now)`,
  `group_into_buckets(&[..],now)`, `bucket_for(at,now)`, `media_kind_from_format`,
  `MediaKind::media_label`, callbacks `exp:tab:* / exp:page:recent:N / exp:rs:ID`
  used consistently across tasks. ✅
