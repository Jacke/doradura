# Fixing the "Unknown Track" Filename Issue

## 🎯 Quick fix
Videos download as `Unknown Track.mp4`? **Restart the bot.**

```bash
pkill -f doradura   # or Ctrl+C
cargo build --release
./target/release/doradura
```

## 🤔 Why it happens
Metadata is cached in memory for 24 hours. If an old run cached "Unknown Track", it stays there until the cache expires or the process restarts.

## 📋 Diagnostics
1. Enable debug logs:
```bash
export RUST_LOG=doradura=debug
./target/release/doradura
```
2. Download a video and check logs.

**Healthy:**
```
[INFO] Successfully got metadata for video - title: 'Real Video Title', artist: ''
[INFO] Generated filename for video: 'Real Video Title.mp4'
```

**Problem (old cache):**
```
[DEBUG] Metadata cache hit for URL: ...
[WARN] Both title and artist are empty, using 'Unknown.mp4'
```
If you see a cache hit with empty title → restart to clear cache.

## 🛠️ Other options
- `./scripts/clear_cache.sh` — cleanup script.
- Wait 24 hours — cache expires automatically.
- Try a video never downloaded before — it will not be cached.

## 🔍 Validation after restart
1. Download any video.
2. Check the filename in `~/downloads/`.
3. Logs should show the real title.

Examples of correct names:
```
✅ How to Code in Rust - Tutorial.mp4
✅ Doradura - New Track (2024).mp4
```
If you still see `Unknown.mp4`, the bot was not fully restarted.

## 💡 Code changes that fixed it
1. **Metadata retrieval** (`src/downloader.rs`)
   - Uses `--print "%(title)s"` instead of `--get-title`.
   - Returns an error instead of falling back to "Unknown Track".
2. **Filename handling** (`src/utils.rs`)
   - Escapes special characters correctly, supports Cyrillic.
3. **Logging**
   - Detailed logs for metadata, filename generation, and cache behavior.

## 📞 If the issue persists
- Ensure only one bot process is running: `ps aux | grep doradura`.
- Check yt-dlp version: `yt-dlp --version`.
- Confirm cookies are configured.
- Test yt-dlp directly:
```bash
yt-dlp --print "%(title)s" "https://youtube.com/watch?v=VIDEO_ID"
```

## 📚 Related docs
- `CACHE_ISSUE.md` — cache details
- `FILENAME_FIX.md` — metadata fixes
- `SESSION_SUMMARY.md` — change summary

## ✅ Checklist
- [ ] Bot stopped
- [ ] Rebuilt (`cargo build --release`)
- [ ] Bot restarted
- [ ] Test download renamed correctly

Enjoy proper filenames! 🎉
