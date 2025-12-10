# Fixing Common YouTube Download Errors

## 1) "Please sign in" / "Confirm you're not a bot"
- **Cause:** No cookies provided.
- **Fix:** Export YouTube cookies (`youtube_cookies.txt`) and set `YTDL_COOKIES_FILE`.

## 2) "ios client requires a GVS PO Token"
- **Cause:** Using the iOS client without cookies.
- **Fix:** Update yt-dlp; ensure cookies are present; use dynamic client selection (android when no cookies).

## 3) HTTP 403 / only images download
- **Cause:** Bot detection or missing auth.
- **Fix:** Always use cookies; prefer `android` or `web` client when unauthenticated.

## 4) Metadata returns empty title
- **Cause:** yt-dlp failed to fetch metadata or cache contains old value.
- **Fix:** Restart the bot to clear cache; ensure cookies are valid; retry.

## 5) Large files rejected
- **Cause:** File size exceeds plan limits.
- **Fix:** Use a higher plan or lower quality; limits are enforced by config.

## Recommended setup
1. Install/refresh cookies (see `YOUTUBE_COOKIES.md` or `MACOS_COOKIES_FIX.md`).
2. Keep yt-dlp current: `pip3 install -U yt-dlp`.
3. Run diagnostics: `./test_ytdlp.sh diagnostics`.
4. If download fails, inspect logs for chosen `player_client` and cookie source.

## Useful env vars
- `YTDL_COOKIES_FILE` — path to exported cookies file.
- `YTDL_COOKIES_BROWSER` — browser to auto-extract cookies (chrome/firefox/brave/edge; safari unreliable on macOS).
- `TELOXIDE_API_URL` — use local Bot API if sending large files.

## Troubleshooting checklist
- [ ] Cookies file exists and is fresh.
- [ ] `player_client` logged correctly (android without cookies, web/ios with cookies).
- [ ] yt-dlp updated.
- [ ] Retry download after clearing cache/restart.

Following these steps resolves the recurring YouTube download errors. ✅
