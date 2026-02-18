# Railway Deployment Guide for Telegram Bot API with Persistent Storage

## Overview

This guide will help you configure a Local Telegram Bot API Server on Railway with a **persistent volume** for storing files up to 2GB.

## What does persistent storage give you?

✅ Files up to **2GB** (instead of the official API's 20MB limit)
✅ Files **persist** across restarts
✅ **Fast access** to files via direct copy
✅ **Fallback** to api.telegram.org on failure

## Cost

Railway Volume: **~$5-10/month** per 1GB storage
(Exact price depends on region and usage)

---

## Step-by-step instructions

### Step 1: Create a Volume on Railway

1. Open Railway Dashboard: https://railway.app
2. Select the Bot API project (or create a new one)
3. Go to the **Variables** section
4. Click **New Variable** → **Volume**
5. Volume settings:
   - **Name:** `telegram-bot-api-data`
   - **Mount Path:** `/telegram-bot-api`
   - **Size:** 1GB (can be increased later)

### Step 2: Configure environment variables

In Railway Dashboard → Variables, add:

```bash
# Required variables (should already be set)
TELEGRAM_API_ID=<your_api_id>
TELEGRAM_API_HASH=<your_api_hash>
TELEGRAM_HTTP_PORT=8081

# NEW variable for the main bot
BOT_API_DATA_DIR=/telegram-bot-api
```

**Important:** `BOT_API_DATA_DIR` must be set in the **main bot**, not in the Bot API server!

### Step 3: Deploy the updated configuration

```bash
# 1. Commit the changes
git add bot-api/
git commit -m "feat: add persistent volume support for Bot API"

# 2. Push to Railway
git push railway main

# 3. Railway will automatically rebuild the container with the volume
```

### Step 4: Verify

After deploying, check the Bot API logs:

```
Starting Telegram Bot API with persistent storage...
Data directory: /telegram-bot-api
```

If you see these lines — everything is working! ✅

---

## Testing

### Test 1: Uploading a large file

1. Send a video to the bot (>20MB)
2. Try to make a clip/cut
3. Check the logs — should use direct copy:

```
📂 Local Bot API: attempting direct file copy from /telegram-bot-api/...
✅ File exists locally, copying directly...
✅ File copied successfully
```

### Test 2: Fallback to api.telegram.org

1. Send a file <20MB
2. If the file is not found on the Local API:

```
⚠️ File not found on local Bot API server, falling back to api.telegram.org
```

This is normal — the bot will automatically download from the official API.

---

## Architecture

### Current setup (with volume)

```
User → Telegram → Railway Bot API → Volume (/telegram-bot-api)
                         ↓
                    Main Bot (direct copy)
                         ↓
                    Processing ✅
```

### Fallback setup (without volume or on 404)

```
User → Telegram → Railway Bot API → ❌ 404 Not Found
                         ↓
                    Main Bot → Fallback to api.telegram.org
                         ↓
                    Download via HTTP ✅
```

---

## Environment Variables

### In the Bot API server (Railway)

```bash
TELEGRAM_API_ID=<your_api_id>
TELEGRAM_API_HASH=<your_api_hash>
TELEGRAM_HTTP_PORT=8081
```

### In the main bot (Railway/VPS)

```bash
BOT_API_URL=https://telegram-bot-api-production-d892.up.railway.app
BOT_API_DATA_DIR=/telegram-bot-api  # ← IMPORTANT!
```

**Note:** If `BOT_API_DATA_DIR` is not set, the bot will use HTTP fallback.

---

## Volume Monitoring

### Checking disk usage

In Railway Dashboard → Metrics you can view:
- Volume usage (GB)
- I/O operations
- Cost

### Cleaning up old files

Telegram Bot API automatically deletes old files after 1 hour.
You can also configure manual cleanup:

```bash
# SSH into Railway container (if needed)
railway run bash

# Check size
du -sh /telegram-bot-api

# Delete old files (>24h)
find /telegram-bot-api -type f -mtime +1 -delete
```

---

## Troubleshooting

### Issue: "BOT_API_DATA_DIR not set"

**Fix:** Set the environment variable in the **main bot**:
```bash
BOT_API_DATA_DIR=/telegram-bot-api
```

### Issue: "File not found" (404)

**Causes:**
1. Volume not mounted — check Railway Dashboard
2. File already deleted by Telegram (>1 hour)
3. Permissions issue — check Bot API logs

**Fix:** The bot will automatically fall back to api.telegram.org

### Issue: Permission denied

**Fix:** The Dockerfile already has a `chown`, but if the issue persists:

```bash
# In entrypoint.sh
chown -R telegram-bot-api:telegram-bot-api /telegram-bot-api
```

### Issue: Volume full (no space)

**Fix:** Increase the volume size in Railway Dashboard or configure auto-cleanup:

```bash
# In cron (if needed)
0 */6 * * * find /telegram-bot-api -type f -mtime +1 -delete
```

---

## Rolling Back

If something goes wrong, you can revert to HTTP-only mode:

1. Remove `BOT_API_URL` from environment variables
2. The bot will automatically switch to `api.telegram.org`
3. File size limit will revert to 20MB

---

## FAQ

**Q: How much does the volume cost?**
A: ~$5-10/month per 1GB on Railway

**Q: Can I increase the size?**
A: Yes, in Railway Dashboard → Volume → Resize

**Q: What if the volume is unavailable?**
A: The bot will automatically fall back to api.telegram.org (20MB limit)

**Q: Do I need to back up the volume?**
A: No, files are temporary (Telegram deletes them after 1 hour)

**Q: Can I use S3 instead of a volume?**
A: Telegram Bot API does not support S3 directly, only local filesystem

---

## Useful Links

- [Railway Volumes Documentation](https://docs.railway.app/reference/volumes)
- [Telegram Bot API Documentation](https://core.telegram.org/bots/api)
- [aiogram/telegram-bot-api Docker Image](https://hub.docker.com/r/aiogram/telegram-bot-api)

---

## Support

If you run into issues, check:
1. Bot API server logs in Railway
2. Main bot logs
3. Railway Dashboard → Metrics → Volume usage

Found a bug? Open an issue on GitHub!
