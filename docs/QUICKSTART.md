# 🚀 Quickstart with YouTube Cookies

## ✅ Cookies are already set up

The `youtube_cookies.txt` file is created and ready to use.

## Run the bot

### Option 1: Environment variable

```bash
# Set the env var
export YTDL_COOKIES_FILE=youtube_cookies.txt

# Run the bot
cargo run --release
```

### Option 2: .env file (recommended)

```bash
# 1. Create .env from the example
cp .env.example .env

# 2. Edit .env and add TELOXIDE_TOKEN
nano .env

# 3. Make sure YTDL_COOKIES_FILE=youtube_cookies.txt is present

# 4. Run the bot
cargo run --release
```

### Option 3: Inline at startup

```bash
YTDL_COOKIES_FILE=youtube_cookies.txt cargo run --release
```

## 🔍 Verify the setup

Check that cookies work:

```bash
# Test with yt-dlp
yt-dlp --cookies youtube_cookies.txt --print "%(title)s" "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
```

If the video title is printed, you're good to go. ✅

## ⚠️ Important notes

### Cookie security

- ✅ `youtube_cookies.txt` is already in `.gitignore`
- ✅ Permissions set to `600` (owner read/write only)
- ❌ **NEVER** publish cookies publicly
- 🔄 Refresh cookies every 2–4 weeks

### When cookies expire

Symptoms:
- Error: "Sign in to confirm you're not a bot"
- Error: "This video is unavailable"

Fix:

#### Method 1: Refresh via browser extension

1. Install [Get cookies.txt LOCALLY](https://chrome.google.com/webstore/detail/get-cookiestxt-locally/cclelndahbckbenkjhflpdbgdldlbecc)
2. Open YouTube.com
3. Export cookies → save as `youtube_cookies.txt`
4. Restart the bot

#### Method 2: Use the browser automatically

```bash
# 1. Install deps
pip3 install keyring pycryptodomex

# 2. Download the helper script
curl -o get_cookies.py https://raw.githubusercontent.com/yt-dlp/yt-dlp/master/devscripts/get-cookies.py

# 3. Extract cookies
yt-dlp --cookies-from-browser chrome --cookies cookies.txt https://www.youtube.com/watch?v=dQw4w9WgXcQ

# 4. Replace youtube_cookies.txt
mv cookies.txt youtube_cookies.txt
```

#### Method 3: Manual export

1. Open YouTube in the browser where you're signed in.
2. Export cookies to a `cookies.txt` format file.
3. Save it as `youtube_cookies.txt` in the project root.
4. Restart the bot.

---

Happy downloading! 🎵
