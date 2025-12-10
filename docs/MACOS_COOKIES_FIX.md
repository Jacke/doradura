# 🍎 macOS Cookie Fix

## 🔴 Problem
On macOS, **Chrome and Safari require special permissions** to access cookies:
- **Chrome:** needs Keychain access to decrypt v10 cookies
- **Safari:** needs Full Disk Access to read `~/Library/Containers/com.apple.Safari/`

**Symptoms:**
```
[WARN] ⚠️  NO COOKIES CONFIGURED!
[ERROR] Operation not permitted: '/Users/stan/Library/Containers/com.apple.Safari/...'
```
or
```
Extracted 0 cookies from chrome (8136 could not be decrypted)
ERROR: Only images are available for download
```

Because of this, cookies are **not extracted** and the bot only sees images instead of videos.

---

## ✅ Solution: Export cookies to a file
An exported cookies file **does not require special permissions** and works perfectly.

### Step 1: Install the browser extension

**Chrome:**
1. Open https://chrome.google.com/webstore/detail/get-cookiestxt-locally/cclelndahbckbenkjhflpdbgdldlbecc
2. Click "Add to Chrome"

**Firefox:**
1. Open https://addons.mozilla.org/en-US/firefox/addon/cookies-txt/
2. Click "Add to Firefox"

### Step 2: Sign in to YouTube
1. Open https://youtube.com
2. Sign in to your Google account
3. Play any video to ensure cookies are saved

### Step 3: Export cookies
1. On the YouTube page, click the extension icon.
2. Click "Export" / "Download".
3. Save the file as `youtube_cookies.txt`.

### Step 4: Copy the file into the project directory

```bash
mv ~/Downloads/youtube.com_cookies.txt /Users/stan/Dev/_PROJ/doradura/youtube_cookies.txt
chmod 600 /Users/stan/Dev/_PROJ/doradura/youtube_cookies.txt
```

### Step 5: Run the bot with the script
```bash
cd /Users/stan/Dev/_PROJ/doradura
./run_with_cookies.sh
```
Or manually:
```bash
cd /Users/stan/Dev/_PROJ/doradura
export YTDL_COOKIES_FILE=youtube_cookies.txt
cargo run --release
```

---

## 🔍 Verify
```bash
ls -la /Users/stan/Dev/_PROJ/doradura/youtube_cookies.txt
yt-dlp --cookies youtube_cookies.txt --print "%(title)s" "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
```
If the title prints, **everything works** ✅

---

## 📋 What the logs show
```
[INFO] Using cookies from file: youtube_cookies.txt
[DEBUG] Using player_client: web,ios,tv_embedded (cookies enabled)
```

---

## ⚠️ Important
1. **Cookies expire every 2–4 weeks** — re-export as needed.
2. **Do not commit the file** — it's already in `.gitignore`.
3. **Security:** `chmod 600 youtube_cookies.txt` (owner read/write only).

---

## 🎯 Why this works

| Method          | macOS sandbox | Needs extra rights |
|-----------------|---------------|--------------------|
| Chrome cookies  | ❌ Works poorly | Keychain access    |
| Safari cookies  | ❌ Works poorly | File access        |
| **Cookies file**| ✅ **Works**     | **No**             |

A cookies file is plain text and does not require special permissions.

---

## 🆘 If it still fails
1. Check the file format — it must start with:
```
# Netscape HTTP Cookie File
```
2. Make sure you're signed in to YouTube in the same browser.
3. Try another browser if Chrome fails.
4. Re-export cookies if they are stale.

---

## 🚀 Done!
Restart the bot and try downloading the video again:
```bash
./run_with_cookies.sh
```

---

## 💡 The bot now warns you
When you start **without cookies** on macOS you will see:
```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
⚠️  NO COOKIES CONFIGURED!
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
YouTube downloads will fail with 'bot detection' or 'only images' errors!

🍎 macOS USERS:
   Browser cookie extraction requires Full Disk Access.
   It's MUCH EASIER to export cookies to a file!

   📖 See: MACOS_COOKIES_FIX.md for step-by-step guide

   Quick fix:
   1. Install Chrome extension: Get cookies.txt LOCALLY
   2. Go to youtube.com → login
   3. Click extension → Export → save as youtube_cookies.txt
   4. Run: ./run_with_cookies.sh
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
Now you always know what to do. 🎯
