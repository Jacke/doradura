# 🚀 Local Telegram Bot API Server Setup

Running a local Bot API server allows you to:
- ✅ Send files up to **2 GB** (vs 50 MB via public API)
- ✅ Reduce network latency
- ✅ Gain more control over webhooks

## 📋 Requirements
1. **API ID and API Hash** from Telegram
2. **Docker** (recommended) or a C++ toolchain to build from source

## 🔑 Step 1: Get API ID and API Hash
1. Go to https://my.telegram.org
2. Sign in with your phone number
3. Open **API development tools**
4. Create a new app (or reuse an existing one)
5. Copy `api_id` and `api_hash`

## 🐳 Step 2: Install with Docker (recommended)

### Quick start
1. Create `.env.bot-api` with your credentials:
```bash
API_ID=YOUR_API_ID
API_HASH=YOUR_API_HASH
```

2. Run the server:
```bash
docker run -d \
  --name telegram-bot-api \
  -p 8081:8081 \
  --env-file .env.bot-api \
  -v $(pwd)/bot-api-data:/var/lib/telegram-bot-api \
  aiogram/telegram-bot-api:latest
```

3. Check status:
```bash
docker logs -f telegram-bot-api
```

4. API endpoint: `http://localhost:8081/bot<TELEGRAM_BOT_TOKEN>/METHOD`

### Useful options
- `-p 8081:8081` — expose port
- `-v ./bot-api-data:/var/lib/telegram-bot-api` — persistent data
- `-e TELEGRAM_STAT=1` — enable stats endpoint (exposes port 8082 by default)
- `-e TELEGRAM_VERBOSITY=4` — verbose logging
- `-e TELEGRAM_LOG_FILE=/var/lib/telegram-bot-api/logs/telegram-bot-api.log` — write logs to file inside container

## 🛠 Step 3: Configure the bot to use the local API

Set the environment variable:
```bash
export TELOXIDE_API_URL=http://localhost:8081
```
Or add to `.env`:
```
TELOXIDE_API_URL=http://localhost:8081
```

Then restart the bot:
```bash
cargo run --release
```

## ⚙️ Step 4: Webhooks (optional)

If using webhooks instead of long polling:
```bash
export TELOXIDE_WEBHOOK_URL=https://your-domain.com/webhook
export TELOXIDE_API_URL=http://localhost:8081
```
Ensure your HTTP server forwards Telegram updates to the webhook path.

## 🧪 Step 5: Verify

```bash
curl "http://localhost:8081/bot<TELEGRAM_BOT_TOKEN>/getMe"
```
Expected:
```json
{
  "ok": true,
  "result": {
    "id": 123,
    "is_bot": true,
    "first_name": "Doradura",
    ...
  }
}
```

## 🧹 Maintenance
- Monitor logs: `docker logs -f telegram-bot-api`
- Update image: `docker pull aiogram/telegram-bot-api:latest && docker restart telegram-bot-api`
- Remove container: `docker rm -f telegram-bot-api`

## 🧭 Building from source (alternative)

If you prefer to build manually:
1. Clone https://github.com/tdlib/telegram-bot-api
2. Install dependencies (cmake, g++, openssl, zlib, etc.)
3. Build:
```bash
cmake -DCMAKE_BUILD_TYPE=Release .
cmake --build . --target telegram-bot-api -- -j4
```
4. Run:
```bash
./telegram-bot-api --api-id=<API_ID> --api-hash=<API_HASH> --http-port=8081 --dir=./bot-api-data
```

## 🔒 Security tips
- Keep `api_id` and `api_hash` private.
- Restrict access to port 8081 (firewall or reverse proxy).
- Do not expose the local API publicly without protection.

## 🧾 Troubleshooting
- Port already in use → change `-p 8081:8081` or free the port.
- `401 Unauthorized` → verify bot token.
- Slow responses → check Docker resources or host load.

Happy self-hosting! 🎉
