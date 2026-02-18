# Quick Deploy Telegram Bot API on Railway

## Step 1: Open Railway Dashboard

Go to [railway.app/dashboard](https://railway.app/dashboard)

## Step 2: Create a new service

1. Open your project (or create a new one)
2. Click **"+ New"** → **"GitHub Repo"**
3. Select the `doradura` repository
4. Railway will automatically detect the Dockerfile

## Step 3: Configure the service

In the service settings (Settings):

### General
- **Service Name**: `telegram-bot-api`

### Source
- **Dockerfile Path**: `Dockerfile.bot-api`

### Networking
- Add a **Public Domain** if external access is needed
- Port: `8081` (automatically)

## Step 4: (Optional) Secure configuration

If using `Dockerfile.bot-api.secure`:

### Environment Variables
Add in Settings → Variables:

```
TELEGRAM_API_ID=YOUR_API_ID
TELEGRAM_API_HASH=YOUR_API_HASH
TELEGRAM_HTTP_PORT=8081
```

> **Get API_ID and API_HASH:** https://my.telegram.org/apps

## Step 5: Deploy

Railway will automatically start deployment. Monitor progress in the **Deployments** section.

## Done!

After successful deployment your Bot API will be available at:
```
https://your-service-name.up.railway.app
```

## What's next?

Use this URL in your bot:

```bash
# In the main bot environment variables
BOT_API_URL=https://your-bot-api-service.up.railway.app
```

## Verification

```bash
curl https://your-bot-api-service.up.railway.app/
```

Should return a response from Telegram Bot API.
