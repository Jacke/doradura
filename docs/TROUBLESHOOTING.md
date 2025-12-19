# Railway Deployment Troubleshooting

Common fixes for deployment issues on Railway.

## 🔴 Bot panic: teloxide-core/src/bot.rs:319:43

### Symptoms
```
[ERROR] Panic caught: PanicHookInfo { payload: Any { .. },
  location: Location { file: ".../teloxide-core/src/bot.rs", line: 319, col: 43 }
```

### Cause
Invalid or missing `TELOXIDE_TOKEN`.

### Fix
1. **Check the token in Railway Dashboard:**
   - Variables → `TELOXIDE_TOKEN`
2. **Ensure the token is valid:**
   - Format: `123456789:ABCdefGHIjklMNOpqrsTUVwxyz`
   - Regenerate via [@BotFather](https://t.me/BotFather) if lost
3. **Update via CLI:**
   ```bash
   railway variables --set "TELOXIDE_TOKEN=YOUR_NEW_TOKEN"
   ```
4. **Or via Dashboard:** edit `TELOXIDE_TOKEN`, save; Railway restarts the service automatically.

---

## ⚠️ YouTube downloads fail

### Symptoms
```
[ERROR] ❌ NO COOKIES CONFIGURED - YouTube downloads will FAIL!
```

### Cause
No cookies provided for YouTube authentication.

### Fix
**Option 1: Provide a cookies file (recommended)**
```bash
railway variables --set "YTDL_COOKIES_FILE=youtube_cookies.txt"
```

**Option 2: Use browser extraction**
```bash
railway variables --set "YTDL_COOKIES_BROWSER=chrome"
```
Requires `keyring` and `pycryptodomex` (already in Dockerfile).

**Option 3: Refresh cookies manually**
1. Export fresh cookies from the browser (extensions: "Get cookies.txt LOCALLY" or "cookies.txt").
2. Save as `youtube_cookies.txt`.
3. Commit and push:
   ```bash
   git add youtube_cookies.txt
   git commit -m "chore: update youtube cookies"
   git push
   ```

### Verify
Run diagnostics:
```bash
./test_ytdlp.sh diagnostics
```
Should show cookies configured and available.
