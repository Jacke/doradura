# 🧪 Download Testing Guide

## TL;DR — quick commands

```bash
# 1. System diagnostics (run FIRST)
./scripts/test_ytdlp.sh diagnostics

# 2. If cookies are not set up — automatic setup
./scripts/run_tests_with_cookies.sh

# 3. Download test (requires internet)
YTDL_COOKIES_FILE=./youtube_cookies.txt ./scripts/test_ytdlp.sh download

# 4. Run the bot with fixes
YTDL_COOKIES_FILE=./youtube_cookies.txt cargo run --release
```

## 📋 What changed

### ✅ Integration test suite
- 8 tests to verify functionality
- Automatic diagnostics
- Detailed error messages with fixes

### ✅ Critical bug fixed

**Problem:**
```
ERROR: [youtube] Please sign in
WARNING: ios client requires a GVS PO Token
```

**Fix:**
- Switched `player_client` from `web,ios` to `android`
- Android client does not require a PO Token
- Stable when using cookies

### ✅ Documentation
- `TESTING.md` — full guide
- `QUICK_FIX.md` — 5-minute fix
- `TEST_SUMMARY.md` — detailed report
- This file — quick instructions

## 🎯 Available tests

| Command | What it checks        | Internet |
|---------|-----------------------|----------|
| `diagnostics` | System check           | ❌ |
| `download`    | Audio download         | ✅ |
| `metadata`    | Metadata retrieval     | ✅ |
| `invalid`     | Error handling         | ✅ |
| `all-basic`   | Everything offline     | ❌ |
| `all`         | Full suite             | ✅ |

Full list: `./scripts/test_ytdlp.sh help`

## ⚡ Usage examples

### Offline basic suite
```bash
./scripts/test_ytdlp.sh all-basic
```

### Full suite with cookies
```bash
YTDL_COOKIES_FILE=./youtube_cookies.txt ./scripts/test_ytdlp.sh all
```

### Single test run
```bash
./scripts/test_ytdlp.sh metadata
```

## 🧠 Tips
- Always run `diagnostics` first to catch environment issues.
- Keep `youtube_cookies.txt` fresh.
- Use `run_tests_with_cookies.sh` if cookies are missing.
- Prefer release mode for realistic performance: `cargo run --release`.
