# Codebase Concerns

**Analysis Date:** 2026-02-06

## Tech Debt

**Premium check disabled for testing:**
- Issue: `if true {` bypasses premium check for audio effects
- File: `src/download/audio.rs` (~line 898)
- Why: Temporarily enabled for all users during testing
- Impact: Revenue loss; tier protection bypassed
- Fix approach: Re-enable premium check after testing confirmed

**Web App API incomplete:**
- Issue: 6 TODO fields returning None/0 in webapp responses
- File: `src/telegram/webapp.rs` (lines 526, 548, 551, 685, 783-784)
- Why: Rapid development, webapp launched without full feature parity
- Impact: Frontend cannot display: estimated time, progress, completion timestamps, success/failure counts
- Fix approach: Implement each TODO field with data from queue and DB

**Large Telegram modules:**
- Issue: Several modules exceed 2000-3000 lines
- Files: `src/telegram/menu.rs` (3855 lines), `src/telegram/admin.rs` (3294 lines), `src/telegram/commands.rs` (2940 lines)
- Why: Organic growth without refactoring
- Impact: Hard to navigate, maintain, and test
- Fix approach: Split into sub-modules by feature area

**Main.rs too large:**
- Issue: Entry point handles bot init + queue processing + background task spawning
- File: `src/main.rs` (1374 lines)
- Why: Incremental feature additions
- Impact: Hard to understand startup flow
- Fix approach: Extract background task spawning and queue processor to separate modules

## Known Bugs

**Cookie manager chromedriver broken on Railway:**
- Symptoms: All chromedriver requests timeout after 120s, circuit breaker permanently open
- Trigger: Railway container environment (Chromium hangs)
- File: `tools/cookie_manager.py`
- Workaround: v5.0 strategy reduces cookie dependency (Tier 1 doesn't need cookies)
- Root cause: Chromium in Alpine container may need additional dependencies or configuration

## Security Considerations

**Missing input validation in WebApp:**
- Risk: `PreviewRequest` and `DownloadRequest` deserialized from JSON without URL/format validation
- File: `src/telegram/webapp.rs`
- Current mitigation: Telegram WebApp auth validates user identity
- Recommendations: Add URL format validation and allowed-format whitelist

**pip install --break-system-packages:**
- Risk: Python system packages could be corrupted
- File: `Dockerfile.s6` (pip install line)
- Current mitigation: Container is ephemeral, rebuilt on deploy
- Recommendations: Use Python venv if issues arise

## Performance Bottlenecks

**Download queue O(n) insertion:**
- Problem: Priority insertion scans queue to find correct position
- File: `src/download/queue.rs`
- Measurement: Not measured (likely fine for current scale)
- Cause: VecDeque with linear scan for priority ordering
- Improvement path: BinaryHeap if queue size becomes >100 concurrent tasks

**String allocations in hot paths:**
- Problem: Heavy `format!()`, `.clone()`, `.to_string()` usage throughout
- Files: Prevalent in `src/telegram/menu.rs`, `src/telegram/admin.rs`, `src/telegram/commands.rs`
- Measurement: Not profiled
- Cause: Convenience over performance
- Improvement path: Audit hot paths, use `Cow<str>` or references where possible

## Fragile Areas

**Cookie management system:**
- Why fragile: Depends on external cookie_manager.py + chromedriver + YouTube auth
- Common failures: Chromedriver hangs, cookies expire, YouTube changes auth flow
- File: `src/download/cookies.rs` (1574 lines)
- Safe modification: v5.0 fallback reduces dependency (Tier 1 works without cookies)
- Test coverage: Smoke tests validate cookies every 5 minutes

**YouTube download fallback chain:**
- Why fragile: Depends on yt-dlp nightly builds, YouTube API changes, external PO token server
- Common failures: yt-dlp breaking changes, YouTube client fingerprinting
- Files: `src/download/video.rs`, `src/download/audio.rs`, `src/download/metadata.rs`
- Safe modification: Test with smoke tests after any change
- Test coverage: Smoke tests cover metadata + audio + video downloads

**External service dependencies (single points of failure):**
- Cookie manager at `http://127.0.0.1:4417` - if down, cookie refresh fails
- PO token server at `http://127.0.0.1:4416` - if down, Tier 2 fails
- File: `src/download/cookies.rs`, `src/core/alerts.rs`
- Mitigation: Tier 1 (no cookies) works independently

## Dependencies at Risk

**yt-dlp nightly builds:**
- Risk: No version pinning, nightly could introduce breaking changes between deploys
- File: `Dockerfile.s6` (downloads latest nightly)
- Impact: Download pipeline could break silently
- Migration plan: Pin to specific date-versioned release if stability issues arise

## Test Coverage Gaps

**Critical download modules:**
- What's not tested: `src/download/audio.rs` and `src/download/video.rs` have minimal inline unit tests
- Risk: Download logic changes could break silently
- Priority: Medium (smoke tests cover happy path)
- Difficulty: Requires mocking yt-dlp binary and file system

**Telegram preview module:**
- What's not tested: `src/telegram/preview.rs` (1933 lines) - no visible unit tests for format parsing
- Risk: Format parsing bugs in video quality selection
- Priority: Medium
- Difficulty: Complex JSON parsing logic needs fixture data

**External service failure modes:**
- What's not tested: Cookie manager unavailable, PO token server down, Telegram API rate limiting
- Risk: Unknown behavior when services fail
- Priority: Low (v5.0 fallback handles most cases)
- Difficulty: Requires integration test infrastructure for service simulation

**Configuration validation:**
- What's not tested: `src/core/config.rs` - config loading and validation
- Risk: Invalid config could cause runtime panics
- Priority: Low (Lazy statics panic at startup, which is acceptable)

---

*Concerns audit: 2026-02-06*
*Update as issues are fixed or new ones discovered*
