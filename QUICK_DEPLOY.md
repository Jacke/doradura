# Quick Deploy to Railway 🚂

Fast guide to deploy the Doradura bot to Railway in 5 minutes.

## Method 1: Automatic (recommended)
```bash
railway login
./deploy.sh
```
The script will:
- Create a Railway project
- Ask for required data (Bot Token, Admin ID, etc.)
- Set environment variables
- Deploy the bot

## Method 2: Manual

### Step 1: Login
```bash
railway login
```

### Step 2: Initialize project
```bash
railway init
```
Choose "Create a new project" and name it `doradura-bot`.

### Step 3: Set variables
```bash
# Required
railway variables --set "TELOXIDE_TOKEN=YOUR_BOT_TOKEN"

# Recommended
railway variables --set "YTDL_COOKIES_FILE=youtube_cookies.txt"
railway variables --set "ADMIN_IDS=YOUR_TELEGRAM_ID"
```

### Step 4: Deploy
```bash
railway up
```

## Method 3: via GitHub

### Step 1: Connect GitHub
1. Go to [railway.app](https://railway.app)
2. Create a project
3. Choose "Deploy from GitHub repo"

### Step 2: Configure variables (same as above)

### Step 3: Deploy
Railway builds from GitHub and deploys automatically.

## Verification
- Check logs: `railway logs -f`
- Bot should start without missing-var warnings.
- Test in Telegram: `/start` and a sample download.

Done! 🚀
