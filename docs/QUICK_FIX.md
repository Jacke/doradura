# 🚨 Quick Fix – Railway Variables

## Problem
Bot fails because required variables are missing in Railway.

## ⚡ 2-minute fix

### Step 1: Open Railway Dashboard
1. Go to https://railway.app
2. Sign in
3. Open your project (e.g., **doradura-bot**)

### Step 2: Add variables
1. Click **Variables** in the left menu.
2. Click **+ New Variable**.
3. Add the first variable:
   - Name: `TELOXIDE_TOKEN`
   - Value: `<your_bot_token>`
4. Click **+ New Variable** again.
5. Add the second variable:
   - Name: `YTDL_COOKIES_FILE`
   - Value: `youtube_cookies.txt`
6. Click **Add/Save**.

### Step 3: Wait for restart
Railway auto-restarts the bot in ~10–30 seconds.

### Step 4: Check logs
1. In the same project, click **Deployments**.
2. Open the latest active deployment.
3. Click **View Logs**.
4. Wait 1–2 minutes.

You should see lines showing the token and cookies path are set (no warnings about missing vars).

## 📱 Test
1. Open Telegram.
2. Find your bot.
3. Send `/start`.
4. Try downloading something from YouTube.

**Done!** 🎉

## 🖼️ Quick visual cue
```
├── Your project (doradura-bot)
│   ├── Variables  ← add them here
```

## 📋 Copy/paste values
### Variable 1
```
Name: TELOXIDE_TOKEN
Value: <your_bot_token_from_BotFather>
```
### Variable 2
```
Name: YTDL_COOKIES_FILE
Value: youtube_cookies.txt
```

## ❓ If Dashboard is not working
### Alternative: Railway CLI
If Railway CLI is set up and linked:
```bash
# 1. Ensure you are in the correct directory
cd /path/to/project

# 2. Connect to the project
railway link

# 3. Set variables
railway variables --set "TELOXIDE_TOKEN=<your_bot_token>"
railway variables --set "YTDL_COOKIES_FILE=youtube_cookies.txt"

# 4. Verify
railway variables

# 5. Restart
railway up
```

## ✅ How to confirm it works
Logs should show:
**✅ GOOD:** no warnings about missing `TELOXIDE_TOKEN` or cookies; bot starts.
**❌ BAD:** warnings like `YTDL_COOKIES_FILE not set` or token errors.

## 💡 Tip
The Railway Dashboard is simpler and more reliable than CLI for adding vars. This fix takes 2 minutes—do it now! 🚀
