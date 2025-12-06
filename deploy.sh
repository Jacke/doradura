#!/bin/bash

set -e

echo "🚂 Doradura Railway Deployment Script"
echo "======================================"
echo ""

# Colors for output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Check if Railway CLI is installed
if ! command -v railway &> /dev/null; then
    echo -e "${RED}❌ Railway CLI is not installed${NC}"
    echo "Install it with: brew install railway"
    exit 1
fi

echo -e "${GREEN}✓ Railway CLI found${NC}"

# Check if logged in
if ! railway whoami &> /dev/null; then
    echo -e "${YELLOW}⚠️  Not logged in to Railway${NC}"
    echo "Please login first:"
    echo "  railway login"
    exit 1
fi

echo -e "${GREEN}✓ Logged in to Railway${NC}"

# Check if project exists
if [ ! -f .railway/config.json ]; then
    echo -e "${BLUE}Creating new Railway project...${NC}"
    railway init --name doradura-bot
fi

echo -e "${GREEN}✓ Railway project configured${NC}"

# Prompt for Telegram Bot Token
echo ""
echo -e "${BLUE}📱 Telegram Bot Configuration${NC}"
read -p "Enter your Telegram Bot Token (from @BotFather): " TELEGRAM_TOKEN

if [ -z "$TELEGRAM_TOKEN" ]; then
    echo -e "${RED}❌ Bot token is required${NC}"
    exit 1
fi

railway variables --set "TELOXIDE_TOKEN=$TELEGRAM_TOKEN"
echo -e "${GREEN}✓ Bot token configured${NC}"

# Optional: YouTube cookies
echo ""
echo -e "${BLUE}🍪 YouTube Cookies Configuration (Optional but recommended)${NC}"
echo "Do you have a youtube_cookies.txt file? (y/n)"
read -r has_cookies

if [ "$has_cookies" = "y" ]; then
    if [ -f "youtube_cookies.txt" ]; then
        echo "Encoding cookies to base64..."
        COOKIES_BASE64=$(base64 -i youtube_cookies.txt)
        railway variables --set "YTDL_COOKIES_FILE=youtube_cookies.txt"
        echo -e "${GREEN}✓ YouTube cookies configured${NC}"
    else
        echo -e "${YELLOW}⚠️  youtube_cookies.txt not found in current directory${NC}"
        echo "You can add it later via: railway variables --set \"YTDL_COOKIES_FILE=youtube_cookies.txt\""
    fi
else
    echo "You can extract cookies from browser instead:"
    echo "  railway variables --set \"YTDL_COOKIES_BROWSER=chrome\""
fi

# Optional: Admin IDs
echo ""
echo -e "${BLUE}👤 Admin Configuration (Optional)${NC}"
read -p "Enter your Telegram User ID (leave empty to skip): " ADMIN_ID

if [ ! -z "$ADMIN_ID" ]; then
    railway variables --set "ADMIN_IDS=$ADMIN_ID"
    echo -e "${GREEN}✓ Admin ID configured${NC}"
fi

# Optional: Mini App
echo ""
echo -e "${BLUE}🌐 Telegram Mini App (Optional)${NC}"
echo "Do you want to enable Telegram Mini App? (y/n)"
read -r enable_miniapp

if [ "$enable_miniapp" = "y" ]; then
    railway variables --set "WEBAPP_PORT=8080"
    echo -e "${YELLOW}⚠️  After deployment, set WEBAPP_URL to your Railway domain${NC}"
    echo "Example: railway variables --set \"WEBAPP_URL=https://your-project.railway.app\""
fi

# Deploy
echo ""
echo -e "${BLUE}🚀 Deploying to Railway...${NC}"
echo "This may take a few minutes..."

railway up --detach

echo ""
echo -e "${GREEN}✅ Deployment initiated!${NC}"
echo ""
echo "Next steps:"
echo "1. Check deployment status: railway status"
echo "2. View logs: railway logs"
echo "3. Open dashboard: railway open"
echo ""
echo "Optional configuration:"
echo "  - Set webhook: railway variables set WEBHOOK_URL=\"https://your-project.railway.app/webhook\""
echo "  - Set Mini App URL: railway variables set WEBAPP_URL=\"https://your-project.railway.app\""
echo ""
echo -e "${GREEN}🎉 Your bot should be running soon!${NC}"
