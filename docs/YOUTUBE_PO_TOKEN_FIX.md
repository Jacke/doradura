# Fixing the YouTube "PO Token" Error

## Issue
yt-dlp sometimes reports:
```
WARNING: [youtube] ios client requires a GVS PO Token
ERROR: Please sign in
```

## Root cause
Using the `ios` client without cookies triggers the PO Token requirement.

## Fix
1. **Switch to the Android client when cookies are absent.**
2. **Use web/ios/tv_embedded only when cookies are available.**
3. **Ensure cookies are configured** for protected videos.

## Configuration
- Default `player_client` selection now depends on cookie availability:
  - With cookies: `web,ios,tv_embedded`
  - Without cookies: `android,web`
- Set `YTDL_COOKIES_FILE` or `YTDL_COOKIES_BROWSER` to enable cookies.

## Verification
```bash
./test_ytdlp.sh diagnostics     # should show chosen player_client
./test_ytdlp.sh download        # download works without PO Token errors
```

## Notes
- Keep yt-dlp up to date: `pip3 install -U yt-dlp`.
- If you still see the warning, refresh cookies and rerun.

With the dynamic client choice plus cookies, PO Token errors should be eliminated. ✅
