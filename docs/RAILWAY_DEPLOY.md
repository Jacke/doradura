# Railway Deployment Guide

This guide explains how to deploy the Doradura Telegram bot to Railway.

## Prerequisites
1. Railway account
2. Railway CLI installed locally
3. Telegram Bot Token from [@BotFather](https://t.me/BotFather)
4. YouTube cookies (optional but recommended)

## Step 1: Authenticate with Railway
```bash
railway login
```
Or export your token:
```bash
export RAILWAY_TOKEN=your_railway_token_here
```

## Step 2: Initialize the project
```bash
railway init
```
Or create a project programmatically:
```bash
railway project create doradura-bot
```

## Step 3: Set environment variables

### Required
```bash
railway variables --set "TELOXIDE_TOKEN=your_bot_token_here"
```

### Optional (recommended)
```bash
# YouTube cookies for access
railway variables --set "YTDL_COOKIES_FILE=youtube_cookies.txt"

# Or extract via browser
railway variables --set "YTDL_COOKIES_BROWSER=chrome"

# Admin user IDs
railway variables --set "ADMIN_IDS=your_telegram_id"

# Mini App config (if using web interface)
railway variables --set "WEBAPP_PORT=8080"
railway variables --set "WEBAPP_URL=https://your-domain.railway.app"
```

### Database
Railway creates a volume for SQLite automatically. By default the DB lives in the container; the volume keeps it persistent.

## Step 4: Configure YouTube cookies
If you have `youtube_cookies.txt`:
```bash
base64 youtube_cookies.txt > cookies_base64.txt
railway variables --set "YOUTUBE_COOKIES_BASE64=$(cat cookies_base64.txt)"
```

If you prefer browser extraction:
```bash
railway variables --set "YTDL_COOKIES_BROWSER=chrome"
```

## Step 5: Deploy

### Option A: Using existing Dockerfile
1. Ensure `Dockerfile` builds the binary and runs it.
2. Push the repo.
3. Railway builds and deploys automatically.

### Option B: railway up
```bash
railway up
```
This builds and deploys the current directory.

## Step 6: Verify logs
```bash
railway logs -f
```
Check for messages like:
- Bot started
- Database initialized/migrated
- No "no such column" errors

## Optional: Reset the database
If the DB schema is outdated ("no such column"), remove `/app/database.sqlite` in Railway Shell or restart the deployment to recreate it. See `FIX_DATABASE.md` for details.

## Tips
- Keep cookies fresh (2–4 weeks).
- Use `WEBAPP_PORT/WEBAPP_URL` only if running the Mini App.
- Keep `TELOXIDE_TOKEN` and cookies secret.
