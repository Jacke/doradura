# 🚀 Quick Download Fix

## ❌ Problem
```
ERROR: [youtube] Please sign in. Use --cookies-from-browser or --cookies
```
**Cause:** YouTube requires cookies for downloads.

## ✅ Fix (5 minutes)

### Step 1: Export cookies from the browser

1. **Install** the "Get cookies.txt LOCALLY" extension (Chrome/Firefox):
   - Chrome: https://chrome.google.com/webstore (search for "Get cookies.txt LOCALLY")
   - Firefox: https://addons.mozilla.org
2. **Sign in to YouTube** in the browser.
3. **Export cookies:**
   - Open youtube.com
   - Click the extension icon
   - Press "Export" → "Current domain (youtube.com)"
   - Save as `youtube_cookies.txt` in the project root

### Step 2: Set the environment variable

```bash
# Set the env var
export YTDL_COOKIES_FILE=./youtube_cookies.txt

# Or add it to ~/.zshrc to persist:
echo 'export YTDL_COOKIES_FILE=/Users/stan/Dev/_PROJ/doradura/youtube_cookies.txt' >> ~/.zshrc
source ~/.zshrc
```

### Step 3: Verify

```bash
# Run diagnostics
./test_ytdlp.sh diagnostics

# Expect:
# ✅ Using cookies file: ./youtube_cookies.txt
# ✅ File exists
```

### Step 4: Download test

```bash
# Requires internet
./test_ytdlp.sh download

# On success:
# ✅ File created: "/tmp/doradura_ytdlp_tests/test_audio.mp3"
# ✅ File size: 245632 bytes
```

### Step 5: Restart the bot

```bash
cargo run --release
```

## 📝 Alternative (Linux)

If you're on Linux, you can extract cookies directly from the browser:

```bash
pip3 install keyring pycryptodomex
export YTDL_COOKIES_BROWSER=chrome
./test_ytdlp.sh diagnostics
```

⚠️ **Does NOT work on macOS** because it requires Full Disk Access.

## 🔍 Check current status

```bash
./test_ytdlp.sh diagnostics
```

The output shows:
- ✅ What's installed
- ✅ Versions
- ✅/❌ Whether cookies are set up
- ✅/❌ Whether the system is ready

## 📚 More docs
- `TESTING.md` — full testing guide
- `MACOS_COOKIES_FIX.md` — detailed macOS instructions
- `YOUTUBE_COOKIES.md` — general cookie notes

## ⚡ Common issues

### "Cookies file not found"
```bash
ls -lh youtube_cookies.txt
# If missing, re-export (Step 1)
```

### "Cookies expired"
Cookies last ~1 year. Re-export if errors appear.

### "Download test fails"
```bash
# Check:
1. export YTDL_COOKIES_FILE=./youtube_cookies.txt
2. ls -lh youtube_cookies.txt
3. ./test_ytdlp.sh diagnostics

# If still failing, update yt-dlp:
pip3 install -U yt-dlp
```

## 🎯 Quick checklist

- [ ] Installed "Get cookies.txt LOCALLY"
- [ ] Signed in to YouTube
- [ ] Exported cookies → `youtube_cookies.txt`
- [ ] File in project root: `ls youtube_cookies.txt` ✅
- [ ] Set env var: `export YTDL_COOKIES_FILE=./youtube_cookies.txt`
- [ ] Diagnostics succeed: `./test_ytdlp.sh diagnostics` ✅
- [ ] Download test succeeds: `./test_ytdlp.sh download` ✅
- [ ] Restarted bot: `cargo run --release`
