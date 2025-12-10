# ✅ Fix for "Sign in to confirm you're not a bot"

## 🎯 Changes
1. **Cookie defaults updated** (`src/config.rs`)
   - Default `YTDL_COOKIES_BROWSER` is now `chrome` (uses browser cookies automatically). Previously empty.
2. **Logging improved** (`src/downloader.rs`)
   - Startup warnings if cookies are missing.
   - Logs show which browser/file is used for cookies.
3. **Android client + cookies conflict fixed** (`src/downloader.rs`)
   - Problem: Android client ignored cookies.
   - Solution: choose client dynamically:
     - With cookies: `web,ios,tv_embedded`.
     - Without cookies: `android,web` fallback.
   - Applied to metadata, audio, and video downloads.
4. **Docs updated** (`FIX_YOUTUBE_ERRORS.md`).

## 🔍 Verification
- Run diagnostics: `./test_ytdlp.sh diagnostics` — should report the browser or file in use.
- Download tests now pass for videos that previously required login.

## ✅ Result
YouTube downloads work with cookies by default; clear warnings are shown when cookies are missing.
