#!/bin/bash
# Script to run a local Telegram Bot API server
# Usage: ./start_local_bot_api.sh

set -e

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo -e "${GREEN}🚀 Starting local Telegram Bot API server${NC}"

# Check Docker
if ! command -v docker &>/dev/null; then
    echo -e "${RED}❌ Docker not installed!${NC}"
    echo "Install Docker: https://docs.docker.com/get-docker/"
    exit 1
fi

# Check docker-compose
if ! command -v docker-compose &>/dev/null; then
    echo -e "${RED}❌ docker-compose not installed!${NC}"
    echo "Install docker-compose"
    exit 1
fi

# Check .env.bot-api
if [ ! -f .env.bot-api ]; then
    echo -e "${YELLOW}⚠️  .env.bot-api not found${NC}"
    echo "Creating template .env.bot-api..."
    cat <<'ENV' > .env.bot-api
API_ID=
API_HASH=
ENV
    echo -e "${YELLOW}📝 Please fill .env.bot-api with your data:${NC}"
    echo "   1. Open https://my.telegram.org"
    echo "   2. Get API_ID and API_HASH"
    echo "   3. Edit .env.bot-api"
    exit 1
fi

# Load env
set -a
source .env.bot-api
set +a

# Verify API_ID and API_HASH
if [ -z "$API_ID" ]; then
    echo -e "${RED}❌ API_ID not set in .env.bot-api${NC}"
    exit 1
fi
if [ -z "$API_HASH" ]; then
    echo -e "${RED}❌ API_HASH not set in .env.bot-api${NC}"
    exit 1
fi

# Check running container
if docker ps --format '{{.Names}}' | grep -q '^telegram-bot-api$'; then
    echo -e "${YELLOW}⚠️  Container telegram-bot-api already running${NC}"
    read -p "Stop and restart? (y/n) " ans
    if [[ "$ans" =~ ^[Yy]$ ]]; then
        echo "Stopping existing container..."
        docker-compose -f docker-compose.bot-api.yml down
    else
        echo "Exit..."
        exit 0
    fi
fi

# Create data dir
mkdir -p bot-api-data

# Start server
echo -e "${GREEN}📦 Starting Docker container...${NC}"
docker-compose -f docker-compose.bot-api.yml up -d

# Wait for server
echo -e "${YELLOW}⏳ Waiting 10 seconds for server start...${NC}"
sleep 10

# Health check
if curl -s http://localhost:8081/bot$TELOXIDE_TOKEN/getMe >/dev/null; then
    echo -e "${GREEN}✅ Server is up!${NC}"
    echo "📋 Info:"
    echo "   - Logs: docker logs -f telegram-bot-api"
    echo "   - Stop: docker-compose -f docker-compose.bot-api.yml down"
    echo "🔧 Configure bot:"
    echo "   set TELOXIDE_API_URL=http://localhost:8081"
    echo "   or add to .env:"
    echo "      TELOXIDE_API_URL=http://localhost:8081"
else
    echo -e "${YELLOW}⚠️  Server started but healthcheck did not respond${NC}"
    echo "   It may still be initializing; wait a bit."
    if ! docker ps --format '{{.Names}}' | grep -q '^telegram-bot-api$'; then
        echo -e "${RED}❌ Failed to start server${NC}"
        echo "Check logs: docker logs telegram-bot-api"
        exit 1
    fi
fi
