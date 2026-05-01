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
- [ ] **`cargo-machete`** — find unused dependencies.
  - Effort: 30 min one-shot
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

## How to use this list

- Items are NOT prioritized; ordering within sections is rough impact descending.
- **Don't try to do everything** — pick 1–3 from "high-impact / low-effort" per session.
- Each item should be a separate commit / PR to keep history clean.
- "Big refactors" need their own session — never bundle with feature work.
- When picking up an item, copy/paste the bullet to a session-scoped task list.

Last updated: this session.
