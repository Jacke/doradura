# TODO — doradura improvement backlog

Compiled from session brainstorms, code reviews, and Rust audit. Organized by impact + effort. Not a roadmap — a candidate list.

---

## ⚠️ Pending state right now

- [ ] **v0.50.4** (workspace lint baseline + `unwrap_used` etc.) committed locally, awaiting `!touch .claude/commit-approved` for push.

---

## 🔥 High-impact / low-effort (do these first)

- [x] **Fix Master/Transparent preset duplicate** — Master = slow/CRF14/AAC320k, Transparent = slow/CRF16/AAC256k.
  - Location: `crates/doracore/src/download/source/ytdlp.rs::EncodeParams::for_preset`
  - Effort: 30 min · Caveman finding · ✅ done
- [x] **N+1 query batch — `get_user_video_download_settings()`** — collapses preset + experimental + send_as_document + video_no_caption into one SELECT. Used in `download_phase` builder + `execute` post-download path. Saves ~2 round-trips per video request.
  - Sites: `pipeline.rs:560,990` (was lines 572 + 998 + 1011 — 3 calls → 1)
  - `video.rs:236-255` (send_as_document + video_no_caption) and `video.rs:847-848` (download_subs + burn_subs) batched in v0.50.7 via `VideoDownloadSettings` + new `SubtitleFlags` bundle.
  - `video.rs:72,862` (progress_bar_style + subtitle_style composite) left — already single queries
  - Effort: 1.5–2 h · Win: −50 to −100 ms latency per download · ✅ done
- [~] **`Arc<str>` for hot-path strings** — `display_title` already `Arc<str>`. Converting `file_format_str` / `artist` would cascade into `DownloadStatus` enum (8 variants, 30+ call sites). YAGNI for ~80 allocs/30s download — **deferred**.
- [x] **Sync I/O in async fn** — `read_log_tail` blocking call in `admin/system.rs:372` (handle_botapi_speed_command) wrapped in `spawn_blocking`. send.rs:634 was already in spawn_blocking.
  - Effort: 15 min · ✅ done
- [x] **`request.clone()` → `Arc<DownloadRequest>`** — `pipeline.rs:585` now `Arc::new(builder.build(...))`, `Arc::clone` for spawn instead of struct clone.
  - Effort: 1 h · ✅ done
- [x] **Refactor `highres_recode_opts`** — replaced with typed `EncodeParams` struct that pushes args directly via `cmd.args(...)`.
  - Caveman finding · Effort: 30 min · ✅ done
- [x] **`stripped.split_whitespace()` in `transmux_or_recode_to_mp4`** — eliminated by `EncodeParams::append_to_ffmpeg(&mut cmd)`.
  - Caveman finding · Effort: 30 min · ✅ done

---

## 🟡 Medium-impact features / cleanups

- [ ] **AV1 Lossless mode** — true byte-1:1 for AV1 sources, sent as `.mkv` document. **User explicitly rejected this — skip.**
- [ ] **GH #8 progress for remaining ffmpeg sites** — `apply_speed_to_file`, `voice_effects`, retry paths in `circle.rs`.
  - Effort: 1.5–2 h
- [ ] **GH #5 speed mod for uploaded files** — apply `apply_speed_to_file` to user uploads (currently downloads only).
  - Effort: 1.5 h
- [ ] **GH #4 file info on uploads** — show metadata when user picks an upload.
  - Effort: 1 h
- [x] **GH #14 queue depth + wait time metrics** — queue depth (high/medium/low) was already in `generate_health_report`; added avg-wait line via new `get_histogram_average(QUEUE_WAIT_TIME_SECONDS)` helper. ✅ done
- [ ] **GH #10 rate limiting for conversions** — per-user / per-feature throttling.
  - Effort: 2 h
- [ ] **GH #12 log rotation** — Railway already rotates, low priority.
  - Effort: 30 min
- [ ] **Cache codec detection from preview phase** — preview's `fast_metadata` already knows vcodec. Pass it down to skip the post-download `ffprobe`.
  - Effort: 1 h · Win: −200–500 ms per highres download
- [ ] **Box<dyn DownloadSource> → enum_dispatch** — 3 impls (YtDlp, Http, Instagram). Inline dispatch, removes one heap alloc per call.
  - Effort: 1 h
- [x] **Disk-pressure-aware cleanup** — `crates/dorabot/src/background_tasks.rs::cleanup_oldest_until_threshold` deletes LRU-by-mtime until disk usage <= 75%, runs after each TTL pass when usage > 80%. Skips files <1h old (in-flight). ✅ done
- [~] **Health check refusing new tasks at <2 GB free** — already covered: `pipeline.rs:387-407` enforces 2 GB min for highres (env-configurable `HIGHRES_MIN_DISK_GB`), 500 MB min for everything else (audio + sub-1080p video files are well under 500 MB). User-friendly error already shown. **Effectively complete** — closing.
- [ ] **Multi-instance Postgres advisory lock** for high-res semaphore — current `LazyLock<Arc<Semaphore>>` is process-local; orphan-kill on startup partially closes the gap.
  - Effort: 1.5 h

---

## 🔵 Big refactors (own session each)

- [ ] **Split `crates/dorabot/src/telegram/commands/circle.rs` (2572 LOC)** into `circle/{parser,video_clip,audio_cut,gif,ringtone}.rs`.
  - Effort: 2 h · Win: faster compile, easier maintenance, easier to test pieces
- [ ] **`.unwrap()` audit (812 calls)** — gradually replace with `?`, `.expect("INVARIANT: …")`, `.unwrap_or_default()`. Remove `#![allow(clippy::unwrap_used)]` per file as cleanup progresses.
  - Effort: 2–3 h initial, ongoing
- [ ] **anyhow → thiserror migration in `dorabot`** — 211 `anyhow::*` usages = heap-allocated errors + dyn dispatch. Define typed errors per module.
  - Effort: 1–2 days
- [ ] **Phase 2++ AV1 Lossless = mkv document mode** — out: rejected by user.
- [ ] **Phase: Hetzner / dedicated migration** — research dedicated hosting with `/dev/dri` exposed to enable Intel QSV (AV1 → H.264 via hardware = 5–10× faster). Currently impossible on Railway shared infra. Worth ~$30/mo only if user volume warrants it.
  - Effort: 1–2 days

---

## 🛠️ Tooling / build / hygiene

- [ ] **`.cargo/config.toml` linker speedup** — `lld` on macOS (need `brew install lld` first), `mold` on Linux (Alpine `mold` exists in `main` repo, but production Dockerfile change requires staging build to verify musl/static-link compatibility — too risky to ship blind).
  - Effort: 5 min config + 30 min Dockerfile changes for mold + staging test
- [ ] **`cargo-sweep` weekly cron** — clean target/incremental files older than 14 days. Currently grows to 40+ GB unmonitored.
  - Effort: 5 min setup
- [ ] **`cargo-deny` in pre-commit** — license check, security advisories, banned/duplicate deps.
  - Effort: 1 h
- [x] **`cargo-audit` in CI** — added new `audit` job in `.github/workflows/ci.yml`. `continue-on-error: true` so transitive-dep advisories surface in PR checks without blocking merges. ✅ done
- [ ] **`cargo-llvm-cov` for coverage** — currently no coverage reporting.
  - Effort: 1 h
- [~] **`cargo-machete`** — ran 2026-05-01, found unused deps to drop:
  - `crates/doratui/Cargo.toml`: `r2d2`, `r2d2_sqlite`, `rusqlite`, `uuid`
  - `crates/doracore/Cargo.toml`: `dashmap`, `figment`, `regex` (only `lazy-regex` is used directly), `tower`
  - `crates/dorabot/Cargo.toml`: `bytes`, `fluent-templates`, `hex`, `hmac`, `r2d2`, `r2d2_sqlite`, `refinery`, `select`, `sha2`, `tower`
  - Effort: 15 min — drop one PR, verify `cargo check --workspace` + `cargo test` clean.
- [x] **`rustfmt.toml`** — already exists at repo root with team conventions (max_width=120, edition=2024, reorder_imports, merge_derives, etc.). ✅ done (pre-existing)

---

## 🧪 Testing / QA

- [ ] **`proptest` / `quickcheck`** for user-input parsers — `parse_time_range_secs`, `parse_segments_spec`, `parse_speed_modifier` are perfect candidates.
  - Effort: 4–6 h
- [ ] **`cargo-fuzz`** on URL parser, format-selector logic, time-range parser.
  - Effort: 1 day
- [ ] **`testcontainers`** crate for postgres-backed integration tests — currently rely on a live local Postgres.
  - Effort: 1 day
- [ ] **`criterion` benchmarks** for download pipeline (have `queue_benchmark` already; need encode/parse benchmarks).
  - Effort: 1 day

---

## ⚡ Performance / innovation (top-5 from "best practices")

- [x] **`mimalloc` global allocator** — added to `crates/dorabot/Cargo.toml` (`default-features = false`), `#[global_allocator]` in `crates/dorabot/src/main.rs`. ✅ done
- [ ] **`tokio_util::sync::CancellationToken`** — replace our custom `Arc<AtomicBool>` cancel signal with hierarchical/structured cancellation. Solves edge cases bare bool can't.
  - Effort: 2 h
- [ ] **Newtypes for IDs** — `ChatId(i64)`, `UserId(i64)`, `MessageId(i32)` instead of bare `i64`. Compile-time prevents id-swap bugs.
  - Effort: 4–6 h (touches many sites)
- [ ] **`arc-swap` for read-heavy shared state** — config reads, source registry. Faster than `RwLock<Arc<T>>`.
  - Effort: 1 h
- [ ] **`bytes::Bytes` for zero-copy buffers** in HTTP / file I/O paths.
  - Effort: 2 h

---

## 📊 Observability

- [ ] **Migrate `log` → `tracing`** with structured fields. Currently mixed (smoke tests on `tracing`, main code on `log`).
  - Effort: 1–2 days
- [ ] **`tokio-console`** for runtime task introspection.
  - Effort: 1 h setup
- [ ] **OpenTelemetry export** — if scaling beyond single-instance.
  - Effort: 1 day
- [ ] **Prometheus metrics endpoint** — already have `/metrics` admin path; expand with download latency histograms, queue depth, encode success rate.
  - Effort: 4 h

---

## 💰 Monetization / growth (parked, low priority)

- [ ] **Watermark in caption** of free-tier videos — "via @doradura_bot — YouTube → Telegram". Free virality.
  - Effort: 30 min
- [ ] **MVP Telegram Stars pay-per-use** — first 5 high-res/day free, then 10 ⭐ per extra. Cheapest possible monetization to validate demand.
  - Effort: 4–5 h
- [ ] **Subscription tiers (Free/Premium/VIP)** — premature optimization until pay-per-use shows demand. Defer.

---

## 🏗️ Architecture (long-term)

- [ ] **Hexagonal architecture** — formalize: doracore = domain, dorabot = telegram adapter, separate http/db adapters. Currently partial.
- [ ] **Worker pool service** — separate container for CPU-heavy encode, doesn't block bot. Useful at scale.
- [ ] **Pre-cache popular videos** — shared dedup if multiple users request the same URL. Useful at scale.
- [ ] **WASM compilation of doracore** — pure-Rust core could run in browser. Speculative.

---

## 🦀 Crate research (2026-05-01)

Synthesized from Reddit r/rust 2024 thread + freestyle.sh 2025 list + Pragmatic Programmers "Ten Favorite Rust Crates". Filtered to crates NOT already in our stack (we have: tokio, anyhow, thiserror, serde, sqlx, reqwest, teloxide, prometheus, tracing, lazy-regex, dashmap, regex, mimalloc, chrono).

### ✅ Top 3 — add next sprint (high ROI / low risk)

| Crate | Why | Where in doradura | Effort | ROI |
|---|---|---|---|---|
| **`bon`** | Typed builders with compile-time required-field check; replaces ad-hoc `expect("title required")` runtime panics | `DownloadConfigBuilder` + new menu/callback state structs in `telegram/menu/*` | 30 min POC | 🟢 High — eliminates a class of "forgot to set field" panics |
| **`strum`** | `EnumString` / `EnumIter` / `Display` / `EnumDiscriminants` derives | `CallbackKind`, `Platform` (iphone/android), `VideoQualityPreset`, `Plan` enums — replace hand-written `parse()` / `as_str()` / `match` arms | 1 h | 🟢 High — removes ~200 LOC of boilerplate, gives `iter()` for `/admin metrics` debugging and i18n key validation |
| **`insta`** | Snapshot tests with reviewable `.snap` files | Menu rendering, FTL locales (4 langs), `format_media_caption`, `format_duration`, callback parser output, error message strings | 1 h setup + 5 anchor snapshots | 🟢 High — locks UX regressions cheaply; the existing snapshot test harness in `tests/bot_snapshots.rs` is hand-rolled and would shrink once `insta` is wired |

### 🟡 Probable value — evaluate during related work

| Crate | Why | Where to apply | Activation trigger | ROI |
|---|---|---|---|---|
| **`pretty_assertions`** | Diff-style assert failure output | All `tests/*.rs` and `#[cfg(test)]` modules | Add now — drop-in dev-dep, no code change | 🟢 5-min payoff |
| **`bytes`** | `Bytes` / `BytesMut` zero-copy buffers for HTTP/file paths | `crates/doracore/src/download/source/http.rs` chunked download body, `send.rs` upload buffers | When we touch HTTP source for any reason | 🟡 Medium — already on main TODO `bytes::Bytes for zero-copy buffers` |
| **`camino`** | `Utf8Path` / `Utf8PathBuf` — UTF-8-guaranteed paths | yt-dlp temp paths in `download_output.file_path`, `actual_file_path` chains in `video.rs` | Next time we audit path handling | 🟡 Medium — eliminates `.to_string_lossy()` clutter |
| **`mockall`** | Mock traits at compile time | Mock `DownloadSource` for unit tests covering pipeline branches without invoking real yt-dlp | When we add the next round of pipeline tests | 🟡 Medium — pairs with proptest |
| **`proptest`** | Property-based / fuzz testing | `parse_time_range_secs`, `parse_segments_spec`, `parse_speed_modifier`, URL canonicalizer, filename sanitizer | Already on main testing list (4-6 h) | 🟡 Medium — fuzzes user-input paths |
| **`testcontainers`** | Docker-based ephemeral Postgres for tests | `tests/*` that currently rely on a live local Postgres; sqlx integration tests | Already on main testing list (1 day) | 🟡 Medium — isolates CI |
| **`serde_with`** | Helpers for `Option<DateTime>`, `#[serde(default)]`-heavy structs, comma-sep lists, etc. | Queue payloads, preview cache structs, share page records | When we touch serde-heavy structs | 🟡 Low–medium |
| **`dotenvy`** | Maintained replacement for the abandoned `dotenv` | `crates/dorabot/src/main.rs` | ✅ Already on `dotenvy` — verified | ✅ Done |

### ❌ Skip — not a fit (with reasoning)

| Crate | Reason to skip |
|---|---|
| **`compact_str`**, **`smallvec`**, **`smallstr`** | Memo `feedback_no_hot_path_micro_opts`: queue benchmark shows 5+ orders of magnitude headroom. Skip micro-opts without flamegraph evidence. |
| **`actix-web`**, **`hyper`** (direct), **`diesel`** | Duplicate of `teloxide` / `reqwest` / `sqlx` we already use. Adding them = two HTTP runtimes, two ORMs — pure liability. |
| **`embassy`**, **`defmt`**, **`heapless`**, **`fixed`**, **`modular-bitfield`**, **`embedded-hal`** | All embedded / `no_std` / MCU. We're a server-side bot on Linux containers — irrelevant. |
| **`wgpu`** | GPU rendering — bot has no graphics path. |
| **`mlua`**, **`v8`** | Embed Lua / V8 scripting — we don't run user-supplied code. |
| **`rppal`** | Raspberry Pi GPIO — wrong form factor. |
| **`tarpc`** | Service-to-service RPC — bot is a single binary, no internal RPC. |
| **`utoipa`**, **`schemars`** | OpenAPI / JSON-schema codegen — we expose a Telegram bot API, not a public REST endpoint. The `/health` and `/metrics` endpoints don't need it. |
| **`hickory`** (formerly trust-dns) | DNS server / resolver — Linux libc resolver via reqwest is sufficient. |
| **`rand`** | std + `getrandom` (transitive via reqwest/teloxide) covers our needs. Pull in only when explicitly required. |
| **`egg-mode`**, **`roux`**, **`serenity`** | Twitter / Reddit / Discord API clients — wrong platform. |
| **`fern`** | Custom log backend — `env_logger` + `tracing` + Railway log capture already covers logging. |
| **`once_cell`** | Replaced by `std::sync::LazyLock` (Edition 2024 stable). |
| **`lazy_static`** | Same — superseded by `LazyLock`. |
| **`derive_more`** | `bon` (builders) + `strum` (enums) + `thiserror` (errors) cover our derive needs without the third-party generic-derive crate. |
| **`async-trait`** | Stable Rust 2024 supports `async fn` in traits natively — no longer needed. |
| **`hex`**, **`hmac`**, **`sha2`** | Currently flagged unused by `cargo-machete` in dorabot. If we end up needing crypto, prefer `ring` or `rustls` ecosystem instead of these stand-alones. |
| **`r2d2`**, **`r2d2_sqlite`** in dorabot/doratui | Flagged unused by `cargo-machete`. We use `sqlx` for postgres + `rusqlite` for sqlite — r2d2 pool is dead weight. Drop in cleanup PR. |
| **`refinery`** in dorabot | Migrations live in `migrations/` and are run via doracore — refinery dep on dorabot is unused. Drop in cleanup PR. |
| **`select`** crate (HTML scraper) | False-positive cargo-machete match for `tokio::select!`. The actual `select` crate is unused — drop in cleanup PR. |
| **`tower`** in dorabot/doracore | Unused middleware framework — we don't compose tower services anywhere. Drop in cleanup PR. |
| **`fluent-templates`** in dorabot | Flagged unused. We use `fluent` directly for FTL i18n. Drop in cleanup PR. |
| **`figment`** in doracore | Flagged unused — we read env vars directly, no config file. Drop in cleanup PR. |
| **`dashmap`** in doracore | Flagged unused (only in dorabot). Drop the doracore-side dep in cleanup PR. |
| **`uuid`** in doratui | Flagged unused — TUI uses `chrono` timestamps, no UUIDs anywhere. Drop in cleanup PR. |

## How to use this list

- Items are NOT prioritized; ordering within sections is rough impact descending.
- **Don't try to do everything** — pick 1–3 from "high-impact / low-effort" per session.
- Each item should be a separate commit / PR to keep history clean.
- "Big refactors" need their own session — never bundle with feature work.
- When picking up an item, copy/paste the bullet to a session-scoped task list.

Last updated: this session.
