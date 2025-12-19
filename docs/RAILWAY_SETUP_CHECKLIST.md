# ✅ Railway Setup Checklist

Step-by-step checks before launching the bot.

## 📋 Pre-Deploy Checklist

### 1. GitHub repository ✅ READY
- [x] Code pushed to GitHub
- [x] Dockerfile created and configured
- [x] `youtube_cookies.txt` in the repo
- [x] Railway configuration prepared

### 2. Railway project
- [ ] Project created on [railway.app](https://railway.app)
- [ ] Repo linked (Deploy from GitHub)
- [ ] First build succeeded

### 3. Environment variables ⚠️ REQUIRED

#### Required
```bash
railway variables --set "TELOXIDE_TOKEN=your_bot_token"
```

#### YouTube (IMPORTANT)
```bash
railway variables --set "YTDL_COOKIES_FILE=youtube_cookies.txt"
```

#### Optional
```bash
# Your Telegram user ID for admin commands
railway variables --set "ADMIN_IDS=your_telegram_id"

# Mini App (if needed)
railway variables --set "WEBAPP_PORT=8080"
railway variables --set "WEBAPP_URL=https://your-project.railway.app"
```

---

## 🚀 Quick Setup (Copy-Paste)

### Option 1: Railway CLI
```bash
railway login
railway link
railway variables \
  --set "TELOXIDE_TOKEN=your_bot_token" \
  --set "YTDL_COOKIES_FILE=youtube_cookies.txt"
railway variables
railway restart
```

### Option 2: Railway Dashboard (recommended)
1. Go to https://railway.app → select your project (`doradura-bot`).
2. Open **Variables**.
3. Add variables:
   - `TELOXIDE_TOKEN` = your bot token
   - `YTDL_COOKIES_FILE` = `youtube_cookies.txt`
   - (optional) `ADMIN_IDS`, `WEBAPP_PORT`, `WEBAPP_URL`

---

## ✅ Ready to deploy when
- All required vars are set (no typos).
- Build completes successfully.
- Logs show no missing-variable warnings.
- `/start` works in Telegram and downloads succeed.
