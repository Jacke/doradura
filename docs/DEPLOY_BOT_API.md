# Deploying Telegram Bot API on Railway

## Available Dockerfile options

1. **Dockerfile.bot-api** - with hardcoded credentials (quick start)
2. **Dockerfile.bot-api.secure** - using ENV variables (recommended)

## Recommended method: Deploy via Railway Web Dashboard

### Step 1: Create a new service

1. Open [Railway Dashboard](https://railway.app/dashboard)
2. Select your project or create a new one
3. Click "New Service" → "GitHub Repo"
4. Select the `doradura` repository

### Step 2: Configure the service

1. In the service settings, find the "Settings" section
2. Change the following parameters:
   - **Service Name**: `telegram-bot-api`
   - **Dockerfile Path**: `Dockerfile.bot-api`
   - **Custom Start Command**: (leave empty, command is already in Dockerfile)

### Step 3: Configure ports

1. In "Settings" → "Networking"
2. Add a public domain (if external access is needed)
3. Make sure port 8081 is exposed

### Step 4: Deploy

1. Railway will automatically start deployment after configuration
2. Monitor logs in the "Deployments" section
3. After successful deployment the service will be available

## Configuration

The service is configured with the following parameters:
- **API ID**: YOUR_API_ID (obtain at https://my.telegram.org)
- **API Hash**: YOUR_API_HASH
- **HTTP Port**: 8081
- **Mode**: --local

## Verifying the deployment

After deployment, check:

```bash
curl https://your-service-url.railway.app/
```

Or check Railway logs for successful startup messages.

## Alternative method: Railway CLI (if available)

```bash
# Make sure you are logged in
railway login

# Create a new project or connect to existing
railway link

# Deploy with Dockerfile specified
railway up --dockerfile Dockerfile.bot-api
```

## Using with the main bot

After deployment, update the environment variable in the main bot service:

```bash
BOT_API_URL=https://your-bot-api-service.railway.app
```

## Important notes

Security: API ID and Hash are hardcoded in the Dockerfile. For production it is recommended to:

1. Use Railway environment variables
2. Create a separate Dockerfile that accepts ENV variables
3. Configure secrets in Railway Dashboard

Persistence: Bot API data is stored in the container. To persist data between deployments, configure Railway Volumes.
