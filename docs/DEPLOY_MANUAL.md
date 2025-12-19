# Manual Railway Deployment (Step-by-Step)

Simple web-based deployment if Railway CLI is not convenient.

## 🚀 Quick start (5 minutes)

### Step 1: Prepare repo
```bash
git add .
git commit -m "Prepare for Railway deployment"
git push
```

### Step 2: Create project on Railway
1. Open [railway.app](https://railway.app)
2. Click **New Project**
3. Choose **Deploy from GitHub repo**
4. Select the `doradura` repository
5. Railway detects the `Dockerfile` and starts building

### Step 3: Set environment variables
Open the project → **Variables** and add:

**Required**
```
TELOXIDE_TOKEN = <your_bot_token>
```

**Recommended**
```
YTDL_COOKIES_FILE = youtube_cookies.txt
ADMIN_IDS = <your_telegram_id>
```

**Optional for Mini App**
```
WEBAPP_PORT = 8080
WEBAPP_URL = https://your-project.railway.app
```

### Step 4: Get domain
1. Railway Dashboard → **Settings**
2. Under **Networking**, click **Generate Domain**
3. Copy the URL (e.g., `doradura-bot-production.up.railway.app`)

### Step 5: Update WEBAPP_URL (if Mini App is used)
Set `WEBAPP_URL` to the generated domain, then redeploy or restart.

### Step 6: Verify
- Check **Deployments** → latest build → logs: no missing-variable warnings.
- Test the bot in Telegram (`/start` and a sample download).

Done! 🚀
