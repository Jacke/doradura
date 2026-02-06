# External Integrations

**Analysis Date:** 2026-02-06

## APIs & External Services

**Telegram Bot API:**
- Primary communication channel for all user interactions
  - SDK/Client: teloxide 0.17 - `Cargo.toml`
  - Auth: Bot token via `TELOXIDE_TOKEN` / `BOT_TOKEN` env var
  - Optional: Local Bot API server via `BOT_API_URL` env var
  - File limits: 50MB standard, 5GB with local Bot API
  - MTProto experimental: grammers-client 0.6 - `src/experimental/mtproto/`
  - Credentials for local API: `API_ID`, `API_HASH`

**YouTube (via yt-dlp):**
- Video/audio download and metadata extraction
  - Integration: yt-dlp binary (nightly builds) - `src/download/ytdlp.rs`
  - Auto-update: Every 6 hours - `src/main.rs`
  - JS runtimes: Deno (primary), Node.js (fallback) - `Dockerfile.s6`
  - Download strategy v5.0:
    - Tier 1: No cookies, `android_vr,web_safari` clients
    - Tier 2: Cookies + PO token fallback
  - Binary path: `YTDL_BIN` env var

**Downsub (Subtitle/Summary Service):**
- Video summaries and subtitle generation
  - Integration: gRPC via Tonic - `src/downsub.rs`
  - Endpoint: `DOWNSUB_GRPC_ENDPOINT` env var
  - Services: GetSummary, GetSubtitles, CheckHealth
  - Timeout: 10 seconds - `src/core/config.rs`
  - Protocol: Protobuf messages (UserContext, MediaReference, SummaryRequest)

## Data Storage

**SQLite:**
- Primary data store for all persistent data
  - Connection: `DATABASE_PATH` env var (default: `database.sqlite`)
  - Client: rusqlite 0.32 + r2d2 0.8 connection pool - `src/storage/db.rs`
  - Migrations: Refinery (V1-V22) - `migrations/`
  - Tables: users, subscriptions, charges, analytics, download_history, error_logs, video_clips, cuts, cookies_sessions, uploads, task_queue

**File Storage:**
- Local filesystem for temporary downloads and media processing
  - Download dir: configured via env var
  - Cleanup: after successful send to Telegram

**Caching:**
- In-memory cache for user data and metadata - `src/storage/cache.rs`
- No external cache service (no Redis)

## Authentication & Identity

**YouTube Auth:**
- Cookie-based authentication for restricted content
  - Cookie file: `YTDL_COOKIES_FILE` env var - `src/core/config.rs`
  - Cookie extraction: headless Chromium + ChromeDriver - `tools/cookie_manager.py`
  - Browser support: chrome, firefox, safari, brave, chromium, edge, opera, vivaldi
  - Cookie manager server: `http://127.0.0.1:4417`
  - Validation: every 5 minutes - `src/main.rs`

**YouTube PO Token Server:**
- bgutil HTTP server for YouTube challenge solving
  - Endpoint: `http://127.0.0.1:4416`
  - Used in: Tier 2 fallback only
  - Framework: Node.js/npm based - `Dockerfile.s6`

**Telegram WebApp Auth:**
- HMAC-SHA256 validation for Mini App data
  - Implementation: `src/telegram/webapp_auth.rs`
  - Standard Telegram WebApp validation protocol

## Proxy Services

**Cloudflare WARP:**
- Free proxy for YouTube access when direct fails
  - Config: `WARP_PROXY` env var (socks5 format)
  - Used as fallback in proxy chain

**Custom Proxy List:**
- File-based proxy configuration
  - Config: `PROXY_FILE` env var (one proxy per line)
  - Strategies: round_robin, random, weighted, fixed - `src/core/config.rs`
  - Health scoring: `PROXY_MIN_HEALTH` threshold - `src/core/config.rs`
  - Dynamic updates: `PROXY_UPDATE_URL` env var
  - Rotation: `PROXY_ROTATION_ENABLED` env var

## Monitoring & Observability

**Prometheus Metrics:**
- Custom metrics collection - `src/core/metrics.rs`
  - HTTP server: `METRICS_PORT` (default: 9090)
  - Enable: `METRICS_ENABLED` env var
  - Metrics: download rates, queue depth, error rates, uptime

**Grafana:**
- Dashboard provisioning - `grafana/provisioning/`

**AlertManager:**
- Alert routing configuration - `alertmanager.yml`

**Custom Alerts:**
- Telegram notifications to admin - `src/core/alerts.rs`
  - Thresholds: error rate, queue depth, retry rate
  - Enable: `ALERTS_ENABLED` env var

## Payments

**Telegram Stars:**
- In-app payment for subscriptions
  - Premium: `PREMIUM_PRICE_STARS` (default: 350) - `src/core/config.rs`
  - VIP: `VIP_PRICE_STARS` (default: 850) - `src/core/config.rs`
  - Period: 30 days
  - Handler: `src/telegram/handlers.rs` (payment + pre_checkout)

## CI/CD & Deployment

**Railway Platform:**
- Production hosting
  - Config: `railway.toml`
  - Build: `Dockerfile.s6` with BuildKit cache mounts
  - Auto-deploy: on git push to main

**Docker:**
- Multi-stage build (Rust builder + Alpine runtime)
  - Process supervision: s6-overlay v3.2.0.2
  - Services: doradura binary, bgutil server, cookie manager

**GitHub Actions:**
- CI pipeline - `.github/workflows/ci.yml`
  - Tests, formatting, linting, smoke tests

## Environment Configuration

**Development:**
- Required: `TELOXIDE_TOKEN`, `DATABASE_PATH`
- Secrets: `.env` file (gitignored)
- Template: `.env.example`
- Staging: `.env.staging`

**Production (Railway):**
- Environment variables in Railway dashboard
- Database: SQLite file in persistent volume (`/data/`)
- Fallback: `/app/database.sqlite` if `/data` not writable

## Webhooks & Callbacks

**Incoming:**
- Telegram Bot API updates (long polling, not webhooks in current setup)
- Telegram payment pre-checkout queries
- WebApp Mini App data submissions

**Outgoing:**
- Downsub gRPC calls (subtitle summaries)
- yt-dlp HTTP calls (YouTube downloads)
- Proxy server connections
- Cookie manager HTTP calls (refresh)

---

*Integration audit: 2026-02-06*
*Update when adding/removing external services*
