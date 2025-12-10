# Download System Testing

## Integration tests for yt-dlp
The integration suite validates downloading through yt-dlp.

### What is covered
1. ✅ Tooling presence (yt-dlp, ffmpeg, ffprobe)
2. ✅ yt-dlp version check
3. ✅ Cookie configuration
4. ✅ Metadata retrieval
5. ✅ Audio download with diagnostics
6. ✅ Error handling for invalid URLs
7. ✅ Different quality/bitrate checks
8. ✅ Full system diagnostics

### Quick start

#### 1) Full diagnostics (run first)
```bash
cargo test --test ytdlp_integration_test test_full_diagnostics -- --nocapture
```
Shows installed tools, versions, cookie status, and readiness.

#### 2) Basic installation checks
```bash
cargo test --test ytdlp_integration_test test_ytdlp_installed -- --nocapture
cargo test --test ytdlp_integration_test test_ytdlp_version -- --nocapture
cargo test --test ytdlp_integration_test test_cookies_configuration -- --nocapture
```

#### 3) Download tests (require internet)
These are `#[ignore]` and must be run with `--ignored`:
```bash
cargo test --test ytdlp_integration_test test_ytdlp_get_metadata --ignored -- --nocapture
cargo test --test ytdlp_integration_test test_ytdlp_download_audio --ignored -- --nocapture
cargo test --test ytdlp_integration_test test_ytdlp_invalid_url --ignored -- --nocapture
cargo test --test ytdlp_integration_test test_ytdlp_different_qualities --ignored -- --nocapture
```

#### 4) Run everything
```bash
cargo test --test ytdlp_integration_test -- --nocapture --test-threads=1
cargo test --test ytdlp_integration_test --ignored -- --nocapture --test-threads=1  # includes download tests
```

### Setup before testing

#### Step 1: Install tools
```bash
# macOS
brew install ffmpeg
pip3 install -U yt-dlp

# Debian/Ubuntu
sudo apt install ffmpeg
pip3 install -U yt-dlp
```

#### Step 2: Configure cookies (required for YouTube)

**Option A: Cookies file (recommended on macOS)**
1. Install "Get cookies.txt LOCALLY" in Chrome.
2. Log in to youtube.com.
3. Export → save as `youtube_cookies.txt`.
4. Set env var:
```bash
export YTDL_COOKIES_FILE=./youtube_cookies.txt
```

**Option B: From browser (Linux)**
```bash
pip3 install keyring pycryptodomex
export YTDL_COOKIES_BROWSER=chrome
```
⚠️ macOS: browser extraction needs Full Disk Access and is unreliable—use a file instead.

#### Step 3: Verify cookies
```bash
cargo test --test ytdlp_integration_test test_cookies_configuration -- --nocapture
```

### Reading results

**✅ Success example**
```
✓ Using cookies file: ./youtube_cookies.txt
✓ File created: "/tmp/doradura_ytdlp_tests/test_audio.mp3"
✓ Size: 245632 bytes
```

**❌ Cookies missing**
```
ERROR: [youtube] ... Please sign in
```
Fix: export cookies file and rerun.

**❌ PO Token warning**
```
WARNING: [youtube] ios client requires a GVS PO Token
```
Fix: `pip3 install -U yt-dlp` and ensure cookies are set.

**❌ HTTP 403**
Likely bot detection—use cookies and consider a different `player_client`.

### CI/CD usage
Run only the non-ignored tests in CI:
```bash
cargo test --test ytdlp_integration_test test_ytdlp_installed
cargo test --test ytdlp_integration_test test_ytdlp_version
cargo test --test ytdlp_integration_test test_full_diagnostics
```
Download tests should be run manually.

### Adding your own tests
Add functions in `tests/ytdlp_integration_test.rs`:
```rust
#[test]
#[ignore] // if it needs internet
fn test_my_feature() {
    // ...
}
```

### Troubleshooting
- Tests hang → use `--test-threads=1`; kill stray yt-dlp processes.
- Temp files remain → clean `/tmp/doradura_ytdlp_tests/`.
- yt-dlp not found → check PATH; install with `pip3 install yt-dlp`.

### More docs
- `MACOS_COOKIES_FIX.md` — macOS cookie setup
- `YOUTUBE_COOKIES.md` — cookie basics
- `FIX_YOUTUBE_ERRORS.md` — common fixes

### Contact checklist
If tests still fail:
1. Run `test_full_diagnostics` and save output.
2. Check `yt-dlp --version`.
3. Ensure `youtube_cookies.txt` exists and is non-empty.
