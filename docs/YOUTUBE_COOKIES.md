# Configuring YouTube Cookies

YouTube sometimes requires authentication ("Sign in to confirm you're not a bot"). The bot uses browser cookies to bypass this. Below are two approaches: automatic browser extraction and a manual cookies file.

## Option A: Automatic extraction (recommended on Linux)

### 1) Install Python deps
```bash
pip3 install keyring pycryptodomex
```

### 2) Verify extraction works
```bash
yt-dlp --cookies-from-browser chrome --print "%(title)s" "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
```
If the title prints, extraction works.

### 3) Pick a browser (defaults to Chrome)
```bash
export YTDL_COOKIES_BROWSER=firefox   # or safari, brave, edge
```

### 4) Run the bot
```bash
cargo run --release
```

## Option B: Manual cookies file (recommended on macOS)

1. Install the "Get cookies.txt LOCALLY" extension (Chrome/Firefox).
2. Log in to YouTube.
3. Export cookies and save as `youtube_cookies.txt` in the project root.
4. Set the env var:
```bash
export YTDL_COOKIES_FILE=./youtube_cookies.txt
```
5. (Optional) Harden permissions:
```bash
chmod 600 youtube_cookies.txt
```

## Checking the setup
```bash
# Using browser extraction
yt-dlp --cookies-from-browser ${YTDL_COOKIES_BROWSER:-chrome} --print "%(title)s" "https://www.youtube.com/watch?v=dQw4w9WgXcQ"

# Using a cookies file
yt-dlp --cookies youtube_cookies.txt --print "%(title)s" "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
```

## Common errors and fixes

- **"Please sign in" / "only images available"** → Cookies missing or expired. Re-export and retry.
- **macOS permission errors** → Use the cookies file method; browser extraction needs Full Disk Access/Keychain.
- **"ios client requires a GVS PO Token"** → Update yt-dlp and ensure cookies are present.
- **HTTP 403** → Bot detection; always use cookies and prefer the `android` or `web` client profiles.

## Security tips
- Never commit `youtube_cookies.txt` (already in `.gitignore`).
- Refresh cookies every 2–4 weeks.
- Limit file permissions to the current user (`chmod 600`).

## Environment variables
- `YTDL_COOKIES_FILE` — path to exported cookies file.
- `YTDL_COOKIES_BROWSER` — browser for automatic extraction (chrome/firefox/brave/edge/safari*).
  - *Safari extraction on macOS is unreliable; use a file instead.

## Quick verification script
```bash
./scripts/test_ytdlp.sh diagnostics
```
Shows whether cookies are configured and usable.

With cookies in place, YouTube downloads should work reliably. ✅
