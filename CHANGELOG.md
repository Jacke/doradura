# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- **Refactor: split 2243-LOC `cookies.rs` into 7 submodules** (v0.39.3 / doracore 0.23.3) — `crates/doracore/src/download/cookies/`. Zero behavior change; external API byte-identical via `pub use` facade. New layout: `types.rs` (579 — `CookieInvalidReason`, `ParsedCookie`, `CookieValidationResult`, `CookieDetail`, `CookiesDiagnostic`, `diagnose_cookies_content`), `file_ops.rs` (244 — `get_cookies_path` as `pub(super)`, `COOKIES_WRITE_MUTEX`, `update_cookies_from_{base64,content}`, `diagnose_cookies_file`, `log_cookie_file_diagnostics`), `probes.rs` (394 — `validate_cookies`, `validate_age_gated_cookies`, `validate_cookies_detailed`, `needs_refresh` + `_ok` wrappers), `watchdog.rs` (136 — `WatchdogStatus`, `watchdog_check`, `format_watchdog_alert`), `manager.rs` (366 — `CookieManagerClient` + 6 response structs + `report_and_wait_for_refresh`), `instagram.rs` (368 — all IG-specific parse/diagnose/validate/update/load helpers). `mod.rs` is a 20-LOC thin facade declaring submodules + `pub use {module}::*;` for each. All 7 caller files (`background_tasks.rs`, `admin/cookies.rs`, `admin/mod.rs`, `telegram/instagram.rs`, `subscriptions.rs`, `source/instagram.rs`, `source/ytdlp.rs`) compile unchanged — zero `use`-path adjustments downstream. Internal helpers (`get_cookies_path`, `COOKIES_WRITE_MUTEX`, `LEGACY_AUTH_COOKIES` etc.) dropped from `pub` to `pub(super)` to avoid expanding the public surface. Circular-dep avoided by moving `diagnose_cookies_file` into `file_ops.rs` (calls `get_cookies_path`) instead of `types.rs`. 574 doracore + 568 dorabot lib tests green; `cargo clippy --workspace -- -D warnings` clean; `cargo fmt` clean.

### Security
- **Dependabot: close 8/12 open advisories** (v0.39.2) — `cargo update`:
  - `openssl 0.10.76 → 0.10.78` closes **4 HIGH** advisories (buffer overflows / oversized-length reads in `Deriver::derive`, `PkeyCtxRef::derive`, `rustMdCtxRef::digest_final`, PSK/cookie trampoline length checks, PEM password callback bounds) — prod TLS path.
  - `actix-http 3.12.0 → 3.12.1` closes **1 MEDIUM** HTTP/1.1 CL.TE request smuggling — used via actix-web in admin panel.
  - `rustls-webpki 0.103.10 → 0.103.13` closes **2 LOW** name-constraints-on-URI / wildcard-name acceptance — prod HTTPS client validation.
  - `rand 0.9.2 → 0.9.4`, `rand 0.10.0 → 0.10.1` closes **1 LOW** unsoundness with custom logger on `rand::rng()`.
  - **Skipped**: `actix-web-lab 0.23.0` advisory (host header poisoning in redirect middleware) — transitive dev-dep from `teloxide_tests` (git-pinned fork), **not in production bot binary**. Patch requires 0.23 → 0.26 (3 pre-1.0 minor bumps) which would need forking teloxide_tests; defer until the upstream fork updates.
  - 574 doracore + 568 dorabot lib tests still green; `cargo check --workspace` clean.

### Fixed
- **CI Lint: suppress `collapsible_match` in subscriptions.rs** (v0.39.1) — Rust 1.95 clippy now flags 5 `match parts[1] { "X" => { if parts.len() >= N { ... } } ... }` sites in `crates/dorabot/src/telegram/subscriptions.rs:247` (the `cw:` callback dispatcher). Collapsing each arm to `"X" if parts.len() >= N => { ... }` would duplicate guards across 5 near-identical arms — less readable than the nested form. Added `#![allow(clippy::collapsible_match)]` at the subscriptions module scope with a rationale comment mirroring the v0.38.22 TUI fix in `doratui/src/main.rs`. CI Lint (workspace `cargo clippy -D warnings`) now green.

### Added
- **Age-verified cookies health probe + edge-triggered admin notifications** (v0.39.0) — `crates/doracore/src/download/cookies.rs`, `crates/dorabot/src/background_tasks.rs`, `crates/dorabot/src/telegram/admin/cookies.rs`. The existing 5-minute `spawn_cookies_checker` only probed `jNQXAC9IVRw` ("Me at the zoo") — a non-age-gated video — so YouTube's age-verification state silently falling off the cookies jar was invisible until a user attempted an 18+ download (production incident on `PmAI3GvuRkA` Rammstein "Sonne", 2026-04-20/23: *"Sign in to confirm your age"* on every tier). Added `validate_age_gated_cookies()` / `validate_age_gated_cookies_ok()` in `doracore` — mirrors the existing proxy-chain probe skeleton but probes `PmAI3GvuRkA` and raises a fast `bail!("Cookies are not age-verified")` when stderr matches the age-gate phrasing (no point retrying proxy tiers, age-gate is API-side). Rewrote `spawn_cookies_checker` as a 2-state machine: `ProbeState { Unknown, Ok, Fail }` tracked per-probe across ticks; admin is notified only on edge transitions (`Unknown|Ok → Fail` and `Fail → Ok`), eliminating the pre-existing 5-min-interval flood on fail (the 6h cooldown in `notify_admin_cookies_refresh` was a band-aid, now the primary dedup is the state machine itself). Age-gate probe runs only when base cookies are OK — base-auth outages otherwise would spuriously flap `age_state` Lost→Recovered. New typed notification helper `notify_admin_age_gate_state(bot, admin_id, AgeGateTransition::{Lost, Recovered})` — Lost message instructs re-exporting cookies from an age-confirmed browser session and preserves `/update_cookies`; Recovered is a short "probe passes again". New Prometheus gauge `doradura_cookies_age_verified_status` (`1` = age-gate probe passes, `0` = lost). 574 doracore + 568 dorabot lib tests green; `cargo clippy -D warnings` clean.

### Fixed
- **SQLite: bump `busy_timeout` 5s → 30s to un-jam queue during long downloads** (v0.38.25) — `crates/doracore/src/storage/db/pool.rs:58`. Production logs (2026-04-20) showed `Failed to claim next queue task: sqlite claim_next_task` firing every 5s for ~4 minutes straight while a 1080p 1.2 GB download held the SQLite writer slot. WAL allows only one writer at a time, and with `busy_timeout = 5000` other writers (queue reaper, log_request, lease heartbeat) hit `SQLITE_BUSY` the moment download-side writes (progress / metadata / history insert) queued up. Bumped to 30 s so SQLite internally retries under contention instead of surfacing BUSY to Rust; no business-logic change. Also documented the interaction with `CONNECTION_TIMEOUT_SECS = 3s` (r2d2 pool acquire ceiling) in `crates/doracore/src/storage/db/mod.rs:32` so future readers don't confuse the two timeouts. 94/94 storage tests + 574 doracore + 568 dorabot + 22 doratui tests all green.
- **Preview format filter: unhide 720p / 1080p for long videos on local Bot API** (v0.38.24) — `crates/dorabot/src/telegram/preview/formats.rs:13`. The preview layer hardcoded a 2 GB cap on video format sizes (`MAX_VIDEO_FORMAT_SIZE_BYTES`) while the send path on local Bot API supports **5 GB** (`doracore::core::config::validation::max_video_size_bytes`). For long videos, 1080p (~4-5 GB) and 720p (~2.5-3 GB) were silently filtered out of the preview keyboard — user report on a 2h26m Noize MC concert: "480p: 1.40 GB / 360p / 240p / 144p — why no 1080p???". Replaced the hardcoded constant with a call to the shared dynamic ceiling (returns 5 GB when `BOT_API_URL` points at a local server, 50 MB for standard `api.telegram.org`). Preview filter now shows every format the send path can actually upload. Removed the now-unreachable `MAX_VIDEO_FORMAT_SIZE_BYTES` constant and its self-referential test. 568/568 dorabot lib tests green.

### Changed
- **Refactor: decompose `handle_settings_callback` 886→153 LOC** (v0.38.23) — `crates/dorabot/src/telegram/menu/callback_settings.rs`. The LARGEST remaining god-function in the workspace and the last untouched callback dispatcher. 13-prefix if-else chain (`mode:`, `main:`, `ext:`, `subscribe:`, `subscription:`, `language:select_new:`, `language:set:`, `quality:`, `video_send_type:toggle:`, `video:toggle_burn_subs`, `settings:toggle_experimental`, `bitrate:`, `audio_send_type:toggle:`, `subtitle:`, `pbar_style:`, `back:`) extracted into 17 private async helpers. Each helper handles one prefix, preserves every `bot.answer_callback_query`, log line, DB call, i18n call, and keyboard build byte-for-byte. Dispatcher body dropped 886 → **153 LOC** (-83%). File: 920 → 1230 LOC (+310 from helper signatures + docs + `#[allow(clippy::too_many_arguments)]`). Largest helpers (`back:` = 151 LOC, `subtitle:` = 102 LOC, `mode:` = 105 LOC) stayed at current size because their inner nested matches are genuinely multi-branch logic that would only get cosmetic cuts from further extraction. Zero behavior change — 569/569 lib tests pass, cargo clippy `-D warnings` clean. **Final god-function decomposition**: `handle_message`, `handle_menu_callback`, `run_loop`, `handle_cuts_callback`, `handle_videos_callback`, `handle_settings_callback` are all below 200 LOC now.

### Fixed
- **CI Lint: suppress `collapsible_match` in doratui/main.rs** (v0.38.22) — Rust 1.95 clippy added `clippy::collapsible_match` that flagged 14 sites in the TUI settings-menu key dispatcher (pattern: `match key.code { KeyCode::X => { if kind == ... } }` which clippy wants collapsed to `KeyCode::X if kind == ...`). Collapsing would produce 14 duplicate `KeyCode::X` arms with different guards — less readable than the current nested style. Added `#![allow(clippy::collapsible_match)]` at the TUI bin crate module scope with a one-paragraph rationale comment. CI Lint now green.

### Changed
- **Hygiene pass** (v0.38.22) — 5 low-risk cleanups:
  - **3 doratui `.unwrap()` → `.expect(…)` with descriptive messages** — `ui/history.rs:130` (display_idx must index into reversed history), `download_runner.rs:569` (candidates non-empty — checked above), `main.rs:447` (match arm guarantees urls.len() == 1). All 3 were infallible-at-runtime but lint-unsafe; converting to `.expect()` documents the invariant loudly if ever violated.
  - **2 metrics_server.rs `.expect(…)` messages sharpened** — "valid HTTP response" → "building 200 OK metrics response with static header cannot fail" / "building 500 error response with plain-text body cannot fail".
  - **2 production `println!` → `log::info!`** in `doracore/src/download/ytdlp.rs:314,329,333` (yt-dlp version check + force-update diagnostics). Other `println!` sites in doracore confirmed to be inside `#[cfg(test)]` modules — left untouched.
  - **Added `await_holding_lock = "warn"` to `[workspace.lints.clippy]`** in root `Cargo.toml`. Prevents regression — audit confirmed no current violations.
  - **Skipped per-agent false positive**: the audit suggested replacing 13 `deps.clone()` in `telegram/handlers/schema.rs` with `Arc::clone(&deps)`. On inspection, `HandlerDeps` is `#[derive(Clone)]`, not wrapped in `Arc<…>` — the current `.clone()` is idiomatic. Skipped.
- **Refactor: decompose `handle_videos_callback` 382→49 LOC** (v0.38.21) — `crates/dorabot/src/telegram/videos.rs`. 10-arm `match action` dispatcher extracted into 9 private async helpers: `handle_videos_page`, `handle_videos_filter`, `handle_videos_open`, `handle_videos_submenu`, `handle_videos_send`, `handle_videos_delete`, `handle_videos_confirm_delete`, `handle_videos_convert`, `handle_videos_circle_speed`. Each takes `&[&str]` parts + cloned `Arc<…>` dependencies. `cancel`/`close` (3 LOC) and `_` (warn) arms stayed inline. Trailing `if data.starts_with("convert:")` preserved verbatim — it's the dual-entry bridge for direct `convert:*` callbacks. Approach B (inline match + per-arm helper) chosen over new sibling module — the cuts refactor (v0.38.20) just validated this pattern. `videos.rs` file 1480 → 1554 LOC (+74 from helper signatures, expected). 569/569 lib tests green. `cargo clippy -D warnings` clean.
- **Refactor: decompose `handle_cuts_callback` 517→44 LOC** (v0.38.20) — `crates/dorabot/src/telegram/cuts.rs`. The cuts callback dispatcher was a 517-LOC god function with 6-arm `match parts[1]` handling `page / open / send / speed / apply_speed / clip / circle / dur / cancel`. Extracted each arm's body into 8 private async helpers (`handle_cuts_page`, `handle_cuts_open`, `handle_cuts_send`, `handle_cuts_speed`, `handle_cuts_apply_speed`, `handle_cuts_clip`, `handle_cuts_circle`, `handle_cuts_dur`). Dispatcher body dropped 517 → 44 LOC (-92%); file LOC 925 → 991 (+66 from per-helper signatures, expected). Approach B (keep inline match, extract bodies) chosen over Approach A (enum + parser) — the tuple variants would mirror `parts[N]` indexes 1:1 without simplification. `handle_cuts_send` (98 LOC) and `handle_cuts_dur` (107 LOC) slightly exceeded the 80-LOC target because each is genuinely independent logic (document fallback path; position-case math + `tokio::spawn`). `cancel` and `close` arms collapsed into a single `"cancel" \| "close"` arm since both did only `delete_message`. Zero behavior change — 569/569 lib tests pass; cargo clippy `-D warnings` clean.

### Fixed
- **CI: test_format_duration expectation off by one format** (v0.38.13) — `crates/dorabot/tests/search_test.rs:93`. Pre-existing test expected `format_duration(Some(3661))` = `"61:01"`, but the actual impl (`doracore::core::format_media_duration`) correctly rolls past 60 minutes into H:MM:SS, so `3661s = 1:01:01`. Fix is test-only — the production code was right. Surfaced by CI's `cargo test --workspace` (workspace-wide integration-test sweep that local `--lib` runs skip).
- **Geo-blocked videos no longer hang the download queue for 4 minutes** (v0.38.12) — `crates/doracore/src/download/source/ytdlp.rs:710-740`. A user reported a Pussycat Dolls video frozen at 0% progress for 8+ minutes. Railway logs showed the preview layer correctly detecting the geo-block on `[Custom Proxy]` (80.76.33.93, RU region) and falling back to `[Direct]` in 9s — but the download layer then started fresh with `[Custom Proxy]`, hit the same "The uploader has not made this video available in your country" error, and got stuck in the Tier 2 cookies-retry path (which can never unlock a country restriction, only propagate the same error slowly). Fix: added geo-block detection to the proxy-chain fast-fallback branch — when yt-dlp returns `YtDlpErrorType::VideoUnavailable` AND stderr contains "not available in your country" / "not made this video available" / "blocked in your country", the download now skips Tier 2/3 and moves to the next proxy immediately. Preserves existing behavior for non-geo `VideoUnavailable` errors (private / deleted videos) — those still run Tier 2/3 because cookies DO help on the first two cases. Zero yt-dlp arg changes (no Railway smoke test required). 574/574 doracore + 568/568 dorabot lib tests green.

### Changed
- **Refactor: parallel agent pass #2 — 3 more non-overlapping god-fn cuts** (v0.38.14 / v0.38.15 / v0.38.16) — second-round parallel burst. Three Rust-expert subagents in separate git worktrees, each on a disjoint file scope. Results cherry-picked into main (after stashing Agent F's work around an intermediate CI-fix commit). 569/569 lib tests green throughout; cargo clippy `-D warnings` clean.
  - **v0.38.14** — `circle.rs::process_video_clip` **935 → 831 LOC** (-104). Extracted `build_clip_filter_plan` (128 LOC pure compute — composes the entire ffmpeg `filter_complex` match ladder: single-circle / multi-circle-split / ringtone / speed-with-video / speed-without-video / no-op) returning a new `ClipFilterPlan` struct replacing a 5-tuple. Plus `build_circle_sub_filter` (11 LOC — SRT-path escape for ffmpeg filter graph). Pure-compute seam; no I/O, no async. (Agent picked Path A over Path B — Path B's type-state pipeline would have forced threading `session` / `guard` / `status` + `mut` locals through stage structs because several values are re-computed after ffprobe, fighting the pattern.)
  - **v0.38.15** — `handle_message` **915 → 765 LOC** (-150, -16%). Five new `try_intercept_*` helpers: `video_clip_text` (cancel / Loop re-prompt / segments-spec parse + `process_video_clip` spawn), `feedback` (feedback capture), `playlist_import_url` (cancel + import spawn), `vault_setup` (vault setup spawn), `playlist_integrations_import` (integrations import spawn). All `ResponseResult<bool>` pattern. commands/mod.rs: 1408 → 1489 LOC.
  - **v0.38.16** — Phase C of `handle_menu_callback` decomposition: replaced the remaining if-else-if chain on prefix strings with a typed `match kind { ... }`. `CallbackKind` enum expanded 20 → 42 variants covering every callback prefix (typo in a prefix string is now a compile-time rename-across-codebase, not a silent routing miss). New `build_forwarded_query` helper collapses 3 near-identical `CallbackQuery { ... }` struct literals (lyr/ac/ae) to 1 call each; new `try_forward!` macro collapses 10 arms of `if-let-Err-log` boilerplate to 1 line each. `handle_menu_callback` body: 631 → **469 LOC** (-26%). +1 new test (`parses_phase_c_prefixes`).

- **Refactor: parallel agent pass — 3 non-overlapping god-fn cuts** (v0.38.9 / v0.38.10 / v0.38.11) — three Rust-expert subagents ran concurrently in isolated git worktrees, each owning a disjoint file scope. Results cherry-picked into `main` with one small merge resolution on `menu/mod.rs` (agent B branched before Phase A, so its `mod callback_download;` addition collided with later `mod callback_kind; mod callback_preview;` additions — resolved by keeping all three). Two stale imports in `callback_router.rs` (`storage::cache`, `url::Url`) pruned after dl: extraction moved their only use sites.
  - **v0.38.9** — `doratui::run_loop` **572 → 74 LOC** (-87%). Decomposed into 5 helpers: `drain_background_events` (103), `dispatch_pending_spawns` (94), `handle_paste_event` (53), `handle_mouse_event` (72), `handle_key_event` (257). `main.rs` total 1896 → 1976 (+80 from sigs/docs/`#[allow(too_many_arguments)]`). 22/22 TUI tests green.
  - **v0.38.10** — `handle_menu_callback::dl:` branch **194 LOC → delegated single call** to new `callback_download.rs` (234 LOC, 9-param entry point). `handle_menu_callback` body 813 → **631 LOC**. callback_router.rs 841 → 657 LOC. `dl:` did not need the `Option<&MaybeInaccessibleMessage>` param that `pv:` did (it never reads the original message). 568/568 lib tests green.
  - **v0.38.11** — 3 more intercept extractions from `handle_message`: `try_intercept_admin_search` (25 LOC), `try_intercept_new_category_session` (55 LOC), `try_intercept_audio_cut_session` (82 LOC) — all following the `ResponseResult<bool>` pattern from v0.38.8. `handle_message` body **1030 → 915 LOC** (-115, -11%). commands/mod.rs 1357 → 1408 LOC. 568/568 lib tests green.
- **Refactor: 2 intercept extractions from `handle_message`** (v0.38.8) — `crates/dorabot/src/telegram/commands/mod.rs`. Extracted `try_intercept_document_upload` (55 LOC block → 3 LOC call; handles cookies + IG-cookies upload sessions) and `try_intercept_video_clip_audio` (65 LOC block → 3 LOC call; handles VideoNote custom-audio capture + Loop audio-triggered processing). Each helper returns `ResponseResult<bool>` where `true = handled, caller returns early`; replaces inline `if let … { … return Ok(None); }` chains with `if helper().await? { return Ok(None); }`. `handle_message` body: 1126 → **1030 LOC** (-8.5%); commands/mod.rs total: 1325 → 1357 LOC (+32 from helper definitions). Zero behavior change — 568/568 lib tests pass. Step 1 of the broader plan to decompose the text-intercept chain (remaining ~900 LOC) into per-intent helpers.
- **Refactor: extract `pv:` preview callbacks to dedicated module** (v0.38.7) — `crates/dorabot/src/telegram/menu/callback_preview.rs` (NEW, 404 LOC). The `pv:` branch inside `handle_menu_callback` was 369 LOC of preview-message action handling (cancel / set / burn_subs / burn_subs_lang / audio / audio_lang) — nearly 1/3 of the god dispatcher's body. Extracted verbatim into a single-purpose module behind an 8-arg entry point `handle_preview_callback(bot, callback_id, message, chat_id, message_id, data, db_pool, shared_storage)`. Arg shape chosen to avoid `&CallbackQuery` (partial-move conflict with outer `if let Some(data) = q.data {` after the move). `handle_menu_callback` body: 1171 → **813 LOC** (-31%). callback_router.rs file: 1200 → 841 LOC. Zero behavior change — inline logic preserved verbatim, only re-indented and unqualified (`super::main_menu::edit_main_menu` → `edit_main_menu` via `use`). 568/568 lib tests green. Phase A of the broader plan to split `handle_menu_callback` into per-prefix modules; next wedges: `dl:` (194 LOC) and the remaining 22 thin dispatch branches (~300 LOC of `if let Err(e) = handle_X(...).await { log::error!(...) }` boilerplate).
- **Refactor: 3 pure extractions from `process_video_clip`** (v0.38.6) — `crates/dorabot/src/telegram/commands/circle.rs`: extracted `compute_clip_max_len_secs` (13→1 LOC at call site), `pick_clip_status_message` (22→1), `build_clip_output_paths` (22→1). All three are pure functions (no I/O, no async, no shared state captured), directly unit-testable in isolation, and replace ~70 LOC of inline if-else chains inside the 975-LOC god function with 3 one-liners. `process_video_clip` body: 975 → 935 LOC (-40); circle.rs file: 2149 → 2203 (+54 — the helpers are now separate fns with doc comments). Each helper has a one-sentence rustdoc explaining its contract. Zero behavior change — 50/50 circle unit tests pass. Step 1 of the broader plan to decompose `process_video_clip` into a `validate → download → probe → build_ffmpeg → exec → upload` pipeline.
- **Refactor: `metric!` macro collapses 57 `LazyLock` metric declarations** (v0.38.5) — `crates/doracore/src/core/metrics.rs`: new `macro_rules! metric` with 8 arms (Counter / Gauge / IntGauge / CounterVec / GaugeVec / IntCounterVec / Histogram / HistogramVec) replaces the repeated `pub static X: LazyLock<_> = LazyLock::new(|| register_!("id", "help"[, labels/buckets]).expect("register X"));` pattern. Every metric now uses one uniform panic format (`panic!("register {}: {}", stringify!($name), e)`) and it's impossible to forget the `LazyLock` wrapper or mistype a `register_!` macro for a given metric type. Doc comments on individual metrics preserved via `$(#[$attr:meta])*` pattern in each macro arm. Zero behavior change — `init_metrics()` still eagerly touches every static at boot, so panic-on-register semantics are identical. File dropped from 1014 → 994 LOC (-20 net, after paying ~60 LOC for the macro definition; real win is uniformity, not byte count). `cargo check -p doracore -p doradura` + `cargo clippy -p doracore --lib` clean.
- **varlock Phase B — fatal boot-time validation** (v0.38.4) — `Dockerfile`: replaced the non-fatal `varlock load 2>&1 || echo [varlock] validation warning (non-fatal)` in the `init-data` s6 oneshot with a hard `if { ... }` execlineb block that exits non-zero when `varlock load` fails. When schema validation fails at boot the container now refuses to start the bot, dumps the last 30 lines of varlock output to stderr, and s6 keeps retrying `init-data` — turning "silent config drift" into an obvious loud restart loop in Railway logs. Pre-flight: `varlock load` exits 0 against current local env (88 vars, no errors) and Railway injects every `@required=forEnv(production)` var (`TELOXIDE_TOKEN`, `TELEGRAM_API_ID`, `TELEGRAM_API_HASH`, `DATABASE_URL`, `REDIS_URL`). No change to the `doradura-bot` entrypoint — all the other long-run services (telegram-bot-api, bgutil-pot-server, doradura-bot) still see the identical env, but `init-data` now guarantees that env passes the schema before they're allowed to start.
- **varlock Phase C — CI drift check** — new `env-schema` job in `.github/workflows/ci.yml` runs `scripts/check-env-drift.sh` on every push/PR. Pure-grep standalone script (no varlock dep in CI) that diffs `std::env::var("FOO")` call sites in `crates/*/src/` against vars declared in `.env.schema`; fails the build if code reads a var the schema doesn't know about. Intentional exclusions list for POSIX vars (`HOME`). Negative-tested locally: adding `std::env::var("FAKE_DRIFT_TEST_VAR")` to a temp source file makes the script exit 1 with a guided fix message. Phase B (Docker boot-time `varlock run`) still deferred.
- **Env schema drift cleanup (varlock Phase A)** — 16 env vars read by code were missing from `.env.schema` (most critically `ANTHROPIC_API_KEY`, `METRICS_AUTH_TOKEN`, `TELEGRAM_BOT_TOKEN`, `VLIPSY_API_KEY` — all real secrets that were unflagged in `varlock scan` because the schema didn't mark them `@sensitive`). Added with correct annotations (`@sensitive` / `@type=number` / `@type=enum(...)`). Also documented 5 runtime-injected vars (`GIT_BRANCH`, `GIT_COMMIT`, `RAILWAY_GIT_BRANCH`, `RAILWAY_GIT_COMMIT_SHA`, `CONTAINER_START_MS`) in a dedicated section so the schema is a complete catalog of every env var the code reads. `varlock load` now validates against 88 declared vars (was 72) with zero errors. New `scripts/validate-env.sh` wraps `varlock load` for reuse in pre-commit hooks and future CI integration. This is Phase A of a larger varlock integration plan — Phase B (Docker boot-time validation via `varlock run -- doradura`) and Phase C (CI code↔schema drift check) are deferred until this change proves stable.
- **Refactor: typed `CallbackKind` enum replaces grouped prefix conditionals in callback router** (v0.38.3) — new `crates/dorabot/src/telegram/menu/callback_kind.rs` defines `CallbackKind` with `strum::EnumString` deriving `FromStr` from the leading `:`-separated token of callback data. Parse-don't-validate at the boundary, then typed match downstream. Replaces two 4-way and 17-way `data.starts_with("xxx:") || ...` conditionals in `callback_router.rs` with `kind.is_some_and(CallbackKind::is_admin_group)` / `is_settings_group`. Same behavior, but a typo in a prefix string is now a compile-time rename-across-codebase rather than a silent silent routing miss. 6 unit tests covering parse / bare token / unknown / group membership / group disjointness. Router body: −21 LOC net. Single-prefix `else if` arms further down (dl:, pv:, history:, export:, vfx:, vp:, etc.) still match on raw strings — intentionally out of scope for this wedge.
- **Refactor: async Mutex → std Mutex for queue timestamp** (v0.38.2) — `crates/dorabot/src/queue_processor.rs`: `Arc<tokio::sync::Mutex<Instant>>` → `Arc<std::sync::Mutex<Instant>>`. The critical section copies 16 bytes and never `.await`s, so the async mutex was paying its overhead for nothing. Also documented the `active_tasks` / `queue` lock-order invariant on `DownloadQueue.queue` with a comment (visibility left as `pub` to preserve access from the main.rs bin-crate integration tests).
- **Refactor: extracted 2 helpers from `process_video_clip`** (v0.38.1) — `crates/dorabot/src/telegram/commands/circle.rs`. `resolve_clip_source` now owns the Download/Cut source lookup + fallback message_info fetch (~80 LOC extracted, returns `Option<ClipSource>` to preserve the user-facing error paths). `send_clip_as_gif` now owns the GIF dispatch branch (~45 LOC extracted). `process_video_clip` body drops from 1071 → 976 LOC. Behavior-preserving — all 50 circle unit tests still green.

### Added
- **Loop to audio** (v0.38.0) — new "🔁 Loop to audio" button on any downloaded MP4. Click → upload an MP3 (or voice / audio-doc) → receive back an MP4 where your video slice plays on loop for the full duration of the song with the song as the audio track. Typical use: a 5-second reaction clip looped 36 times under a 3-minute song. Implementation piggybacks on the existing `VideoClipSession` + `custom_audio_file_id` infrastructure that was already wired for the circle/video-note flow — a new `OutputKind::Loop` variant, one extra `matches!(VideoNote | Loop)` branch in the audio upload intercept, a new `"loop"` action branch in `downloads/clipping.rs::handle` calling the unchanged `start_session_from_download` helper, and a new `process_loop_to_audio` task that runs `ffmpeg -stream_loop -1 -i video.mp4 -i audio.mp3 -map 0:v:0 -map 1:a:0 -c:v libx264 -preset veryfast -crf 23 -pix_fmt yuv420p -c:a aac -b:a 192k -shortest -movflags +faststart output.mp4`. Re-encoding is mandatory — `-c:v copy` with `-stream_loop` fails on non-keyframe loop boundaries and produces corrupted output. Max audio duration 10 minutes (keeps output ≤50 MB `sendVideo` ceiling). Zero migrations, zero new DB tables. Observability via new `doradura_loop_to_audio_total{outcome}` Prometheus counter (`success` / `audio_too_long` / `audio_too_short` / `video_too_short` / `ffmpeg_failed` / `download_failed` / `send_failed`). Two unit tests on the pure ffmpeg command builder verify `-stream_loop`, `-shortest`, `+faststart`, both `-map` args, input ordering, and the mandatory re-encode codecs

### Fixed
- **Vertical videos no longer stretched to landscape in Telegram** (v0.37.0) — `probe_video_metadata` in `doracore/src/download/metadata.rs` previously asked ffprobe only for `stream=width,height`, which returns *coded* dimensions ignoring display rotation. Portrait videos from phones are typically stored as raw `1920x1080 + rotation=-90` in the container; we were sending Telegram `width=1920, height=1080`, the client trusts those params over the file's display matrix, and rendered everything as 16:9 landscape (stretching the portrait content horizontally). Now ffprobe is called with `stream=width,height:stream_tags=rotate:stream_side_data=rotation -of json`, and the new pure helper `dimensions_from_ffprobe_json` reads rotation from both conventions (legacy `tags.rotate` string / modern `side_data_list[].rotation` int), normalizes via `rem_euclid(360)`, and swaps width↔height for 90°/270° rotated streams. 9 new unit tests cover landscape, native portrait, legacy tag (`90`, `-90`, `180`), modern display matrix, both-present precedence, missing streams, and garbage input. Also halved ffprobe subprocess count: one JSON call instead of two `default=noprint_wrappers` calls. No changes to yt-dlp args — only to the metadata probe that runs *after* download, before `send_video`

### Reverted
- **One-tap download reverted** — preview card restored as the default URL flow. User tested the "skip preview, download immediately" behavior in production and rejected it. The preview card (title, quality options, download button) is a valued part of the UX, not unnecessary friction. The `enqueue_download_tasks` shared helper remains in `helpers.rs` for internal use by `start_download_from_preview`
- **File_id cache observability** (v0.37.0) — **Tier S PRD feature** (Aggressive file_id cache, the underlying canonicalization and cross-user dedup is already shipped in v0.34.1 via commit 406660cc). Added `doradura_file_id_cache_total{source,outcome}` Prometheus counter so we can actually measure the "80%+ requests → 0 seconds" PRD target in production instead of guessing. Wired into every cache lookup in `download/pipeline.rs`:
  - `source=vault, outcome=hit` — audio vault layer served the file_id
  - `source=vault, outcome=send_failed` — vault's file_id expired on the Bot API server (falls through to download_history)
  - `source=download_history, outcome=hit` — cross-user download history served the file_id
  - `source=download_history, outcome=miss` — no cached file_id, falling through to fresh download
  - `source=download_history, outcome=send_failed` — cached file_id expired on the Bot API server
  Hit rate over a window = `sum(hit) / sum(hit+miss)`. Can now be graphed in Grafana alongside `doradura_downloads_total` to compare cache-served vs fresh-downloaded traffic
- **Oversized-file check moves to the downloader layer** — previously the audio size check ran during preview metadata fetch. Now the downloader handles it at ingest. Trade-off: 0s latency before download starts (vs ~3s), but "too large" errors surface slightly later if encountered. The downloader already had this check — the preview-layer one was redundant

### Changed
- **Boilerplate cleanup** (v0.37.0, part of one-tap release) —
  - `PipelineError` replaced its hand-written `impl std::fmt::Display` with `#[derive(thiserror::Error)]` + `#[error("...")]` attributes — 9 LOC deleted, standardized with the other error types in the workspace
  - 9 `LazyLock<Regex>` sites across `commands/mod.rs`, `lyrics/mod.rs`, `vlipsy.rs`, `timestamps/{description_parser,url_parser}.rs`, and `fast_metadata.rs` migrated to `lazy_regex::lazy_regex!` macro — patterns are now validated at compile time instead of `.expect("valid regex")` at first use, and the `LazyLock`/`Regex` imports and `.expect(...)` boilerplate are gone
  - `vlipsy::extract_meta` rewrote its two runtime `Regex::new(&format!(...))` calls (per scrape!) as a single precompiled `META_RE` regex + linear filter on the capture group. Also removed the "patterns are NOT hoisted" comment that explained the old hack
  - Added `lazy-regex` to the `dorabot` crate (already a workspace dep used by `doracore`)
  - Honest note: also considered migrating `SearchSource::{from_code,code,label}` and `Filter::{from_code,code}` to `strum::EnumString + IntoStaticStr` derives but the current `match` blocks are clearer than strum + custom-`serialize` attributes for enums with 5 distinct string mappings; skipped
- **Prune unused workspace dependencies + minor version bumps** (v0.36.17) — ran `cargo udeps` and `cargo tree -d` to collect real data on dead / duplicated dependencies, then:
  - **Removed phantom deps that the code no longer references:**
    - `tonic` + `prost` from `dorabot` (only used by `doracore::downsub`, not by dorabot directly)
    - `tower-http` from `doracore` + `dorabot` (zero references in src/)
    - `shell-escape` from `dorabot` (zero references)
    - `tokio-retry` from `dorabot` (zero references — replaced by internal `core::retry` long ago)
  - **Version bumps that resolved our own duplicate-crate versions:**
    - `thiserror 1.0` → `2.x` — eliminates the workspace-triggered duplicate (transitive v1 still comes in via fluent-templates 0.8 / prometheus 0.14; evaluated upgrading fluent-templates to 0.13 but the API break on `lookup`/`lookup_with_args` is non-trivial and not worth the churn for one transitive dep)
    - `strum 0.26` → `0.27` — eliminates our part of the duplicate (doratui deps pull 0.27 via ratatui)
  - Net: 5 direct deps removed, 2 versions aligned, zero behavior change. All 1125 tests pass. Compile times should improve modestly (`tonic` + `prost` alone used to pull in `axum 0.6`, `base64 0.21`, and `axum-core 0.3` as duplicates of our workspace versions)

- **`DownloadTask` uses `bon::Builder` instead of 3 positional constructors** (v0.36.16) — added `bon = "3"` as a workspace dep. The previous `DownloadTask::new(url, chat_id, message_id, is_video, format, video_quality, audio_bitrate)` / `::with_priority(...)` / `::from_plan(...)` positional constructors (7 / 9 / 8 args each) have been replaced with `#[derive(bon::Builder)]` on the struct. Defaults for `id` (auto UUID), `created_timestamp` (`Utc::now()`), `priority` (`Low`), and `with_lyrics` (`false`) are encoded via `#[builder(default = ...)]`. Migrated **30 call sites across 7 files** (`main.rs`, `download/queue.rs` (16 sites in tests), `core/history.rs`, `telegram/menu/{helpers, callback_router, search}.rs`, `telegram/commands/mod.rs`). Usage:

  ```rust
  let task = DownloadTask::builder()
      .url(url)
      .chat_id(chat_id)
      .is_video(false)
      .format(DownloadFormat::Mp3)
      .audio_bitrate("320k".to_string())
      .priority(TaskPriority::from_plan(plan))
      .build();
  ```

  Field names at the call site make each argument self-documenting — the old `DownloadTask::new(url, chat_id, None, false, DownloadFormat::Mp3, None, Some("320k".to_string()))` required counting positions to know which `None` was `message_id` vs `video_quality`. All 1125 tests pass

- **yt-dlp Tier 1/2/3 closures deduplicated via helper functions** (v0.36.15) — the six download closures in `doracore/src/download/source/ytdlp.rs` (Tier 1/2/3 × audio/video) each inlined a verbatim copy of the same 4-item "runtime/cert/concurrent-fragments" tail (`--js-runtimes deno --no-check-certificate -N N`), and the audio/video format prefixes were copy-pasted across their respective tiers. Extracted three helper functions:
  - `push_js_runtimes_tail(args, cf_str)` — the 3-item common tail + optional `-N N` pair
  - `push_audio_format_args(args, with_thumbnail)` — 6 or 7 audio args depending on tier
  - `push_video_format_args(args, with_merger_postprocessor)` — 3 or 5 video args depending on tier

  All six closures now call these helpers. Argv output is **byte-identical** — pinned by 6 new unit tests in `common_args_tests` that assert the exact slices. Net: ~40 lines of duplication deleted, and any future refactor that accidentally drops or reorders a flag now fails CI. **Per CLAUDE.md, any deploy of this commit still requires a Railway smoke test** against a real YouTube URL; the tests prove the Rust side hasn't drifted but don't replace an end-to-end yt-dlp run

### Fixed
- **Cookie validation report showed red ❌ for legacy cookies on modern YouTube exports** (v0.36.14) — when a user uploaded a fresh cookie export from modern Chrome (which ships `__Secure-3PSID` / `__Secure-3PAPISID` / `LOGIN_INFO` instead of the legacy `SID`/`HSID`/`SSID`/`APISID`/`SAPISID` set), the `/update_cookies` command produced a confusing report:
  - `*Required auth cookies:*` section listed all 5 legacy names as ❌ missing
  - `*Additional cookies:*` section showed the modern cookies as ✅ present
  - Overall verdict: ✅ *Cookies look valid*
  - yt-dlp download test: ✓ passed

  The validator's "valid" verdict was already correct (it accepted `__Secure-*PSID` as a modern-auth substitute), but the report template still hard-coded the legacy names under "Required" and appended them to `auth_cookies_missing` blindly. Users saw a wall of red on cookies that actually worked fine.

  **Fix** in `doracore/src/download/cookies.rs`:
  - Split the old `REQUIRED_AUTH_COOKIES` const into `LEGACY_AUTH_COOKIES` (SID/HSID/SSID/APISID/SAPISID) and `MODERN_AUTH_COOKIES` (__Secure-3PSID/1PSID/3PAPISID/1PAPISID/LOGIN_INFO)
  - `diagnose_cookies_content` now detects which scheme the user's cookies are using (`has_any_modern` vs `has_all_legacy`) and only reports misses from the relevant scheme. A user on the modern scheme never sees legacy names reported as missing.
  - Report header renamed from `*Required auth cookies:*` to `*Authentication cookies:*` since "required" was misleading — both schemes are acceptable.
  - Empty-section guards added: the header is skipped entirely if there are no auth details and no missing cookies (instead of printing a lone "Authentication cookies:" with nothing under it).
  - Three regression tests pinned in `cookies::tests`:
    - `diagnose_modern_youtube_cookies_are_valid_and_not_missing_legacy`
    - `diagnose_legacy_youtube_cookies_are_valid`
    - `diagnose_no_auth_cookies_is_invalid`

### Changed
- **`build_common_args` deduplication + regression tests** (v0.36.13) — in `doracore/src/download/source/ytdlp.rs`, `build_common_args` and `build_common_args_minimal` previously duplicated the 14-arg prefix. `build_common_args` now starts by calling `build_common_args_minimal(...)` and appends the retry/throttle tail, so the two cannot drift apart on the shared prefix. Added three regression tests in a new `common_args_tests` submodule that assert the **exact** argv slice (byte-identical to what we shipped), so any future refactor that silently drops or reorders an arg will fail CI. Addresses the `YtDlpArgsBuilder` audit item in the minimal-risk way per CLAUDE.md — the full Tier 1/2/3 builder rollout still needs a Railway smoke test before touching any more `download/*` files

### ⚠️ Still needs Railway smoke test before deploying
None of these changes alter yt-dlp's actual argv output — the regression tests pin the bytes — but per CLAUDE.md policy, any change in `download/*` files should be smoke-tested against a real YouTube URL on Railway before `git push`. Run:

```bash
railway ssh --service doradura -- sh -c 'yt-dlp -o /tmp/t1.mp3 --newline --force-overwrites --no-playlist --age-limit 99 --fragment-retries 10 --socket-timeout 30 --http-chunk-size 10485760 --extract-audio --audio-format mp3 --audio-quality 0 --add-metadata --embed-thumbnail --cookies /data/youtube_cookies.txt --extractor-args youtube:player_client=default --js-runtimes deno --no-check-certificate "https://youtu.be/jNQXAC9IVRw" 2>&1 | tail -3; ls -lh /tmp/t1.mp3 && echo PASS || echo FAIL; rm -f /tmp/t1.mp3'
```

- **Inline HTML extracted to `include_str!`** (v0.36.12) — three large HTML templates (200+ lines total) previously embedded as `format!(r#"..."#)` strings inside `auth.rs::admin_login_handler`, `public.rs::render_privacy_page`, and `public.rs::render_share_page` were moved to sibling `.html` files in `crates/doracore/src/core/web/html/`: `admin_login.html`, `privacy_layout.html`, `share_page.html`. Loaded at compile time via `include_str!` and templated with plain `.replace("{PLACEHOLDER}", value)`. Benefits: CSS brace escaping (`{{ }}` → `{`) is gone, HTML files get proper editor syntax highlighting, no more wrestling with `format!`'s positional args, zero runtime cost (same `&'static str` baked into the binary). Rust files shrink by ~200 lines of noise

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
