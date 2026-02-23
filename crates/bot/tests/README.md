# Integration Tests

## Quick start
1) System diagnostics (run first):
```bash
./test_ytdlp.sh diagnostics
```
Shows installed tools and missing pieces.

2) Cookies setup (if needed)
If diagnostics shows `❌ Cookies not configured`:
```bash
cat ../QUICK_FIX.md
```

3) Download test
```bash
./test_ytdlp.sh download   # requires internet
```

## Available tests
| Command | Description | Internet |
|---------|-------------|----------|
| `./test_ytdlp.sh diagnostics` | Full system diagnostics | ❌ |
| `./test_ytdlp.sh install` | Check yt-dlp/ffmpeg installation | ❌ |
| `./test_ytdlp.sh version` | Check yt-dlp version | ❌ |
| `./test_ytdlp.sh cookies` | Check cookies configuration | ❌ |
| `./test_ytdlp.sh metadata` | Fetch video metadata | ✅ |
| `./test_ytdlp.sh download` | Audio download test | ✅ |
| `./test_ytdlp.sh invalid` | Invalid URL handling test | ✅ |
| `./test_ytdlp.sh quality` | Different quality/bitrate test | ✅ |
| `./test_ytdlp.sh all-basic` | All offline tests | ❌ |
| `./test_ytdlp.sh all-download` | All download tests | ✅ |
| `./test_ytdlp.sh all` | ALL tests | ✅ |

## Running via cargo
```bash
cargo test --test ytdlp_integration_test test_full_diagnostics -- --nocapture
cargo test --test ytdlp_integration_test -- --nocapture --test-threads=1
cargo test --test ytdlp_integration_test --ignored -- --nocapture --test-threads=1
```

## Test structure
- `ytdlp_integration_test.rs` — main integration tests
- helper scripts: `test_ytdlp.sh`, `run_tests_with_cookies.sh`
