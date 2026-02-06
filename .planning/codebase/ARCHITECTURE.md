# Architecture

**Analysis Date:** 2026-02-06

## Pattern Overview

**Overall:** Layered Modular Monolith with Event-Driven Queue Processing

**Key Characteristics:**
- Telegram Bot as primary interface (long polling)
- Priority-based download queue with async processing
- SQLite persistence with connection pooling
- Background task scheduling (health checks, metrics, alerts)
- Multi-tier download fallback chain (no-cookies -> cookies+PO token)

## Layers

**API Layer (Telegram Interface):**
- Purpose: Receive and route user messages, commands, callbacks
- Contains: Handler tree (dptree-based), command dispatch, menu UI
- Location: `src/telegram/`
- Depends on: Business Logic layer, Storage layer
- Used by: Teloxide dispatcher (entry point)

**Business Logic Layer (Download Pipeline):**
- Purpose: Download orchestration, media processing, metadata extraction
- Contains: Video/audio download, queue management, cookie handling, proxy selection
- Location: `src/download/`
- Depends on: Storage layer, Core infrastructure, external tools (yt-dlp, ffmpeg)
- Used by: Telegram handlers

**Core Infrastructure Layer:**
- Purpose: Configuration, error handling, rate limiting, metrics, alerts
- Contains: Config management, error types, rate limiter, metrics, subscriptions
- Location: `src/core/`
- Depends on: Nothing (foundational)
- Used by: All other layers

**Storage Layer (Data Access):**
- Purpose: SQLite operations, caching, migrations
- Contains: Database operations (r2d2 pool), in-memory cache, backup utilities
- Location: `src/storage/`
- Depends on: Core infrastructure
- Used by: Download pipeline, Telegram handlers, Background tasks

**Supporting Modules:**
- `src/conversion/` - Media format conversion (video, image, document)
- `src/timestamps/` - Video timestamp and chapter parsing
- `src/downsub.rs` - gRPC gateway for subtitle summaries
- `src/smoke_tests/` - Health checks and monitoring
- `src/i18n.rs` - Multi-language support (fluent templates)
- `src/experimental/mtproto/` - Telegram MTProto client (experimental)

## Data Flow

**User Message -> Download Flow:**

1. User sends YouTube URL to Telegram bot
2. Teloxide dispatcher routes to message handler - `src/telegram/handlers.rs`
3. Handler detects URL, creates DownloadTask - `src/telegram/handlers.rs`
4. Task added to priority queue (High/Medium/Low by plan) - `src/download/queue.rs`
5. Task persisted to SQLite for durability - `src/storage/db.rs`
6. Background queue processor picks task (semaphore-limited) - `src/main.rs`
7. Download via yt-dlp with fallback chain - `src/download/video.rs`, `src/download/audio.rs`
   - Tier 1: No cookies, `android_vr,web_safari` clients, Deno runtime
   - Tier 2: Cookies + PO token (fallback)
8. Media processed (conversion, thumbnail) - `src/conversion/`
9. File sent to user via Telegram - `src/download/send.rs`
10. Task marked completed in database

**State Management:**
- SQLite database for persistent state (users, tasks, history, subscriptions)
- In-memory priority queue (VecDeque) synchronized with DB
- In-memory cache for user data and metadata - `src/storage/cache.rs`
- Rate limiter state in memory - `src/core/rate_limiter.rs`

## Key Abstractions

**HandlerDeps (Dependency Injection):**
- Purpose: Shared state passed to all Telegram handlers
- Contains: Bot instance, DB pool, queue, rate limiter, metrics
- Location: `src/telegram/handlers.rs`
- Pattern: Struct injected via teloxide dependency injection

**DownloadQueue (Task Processing):**
- Purpose: Priority-based async download processing
- Contains: VecDeque with priority insertion, semaphore concurrency control
- Location: `src/download/queue.rs`
- Pattern: Producer-consumer with DB-backed persistence

**AppError (Error Handling):**
- Purpose: Domain-specific error types with categorization
- Contains: Error variants, categories, metrics tracking
- Location: `src/core/error.rs`
- Pattern: thiserror-derived enum with `AppResult<T>` type alias

**Operation (Multi-Step State Machine):**
- Purpose: Manage multi-step user interactions
- Contains: State transitions, rollback, message formatting
- Location: `src/telegram/operation.rs`
- Pattern: State machine with NotStarted -> InProgress -> Completed

**Configuration (Environment-Driven):**
- Purpose: Centralized config from environment variables
- Contains: Lazy statics with defaults, validation at startup
- Location: `src/core/config.rs`
- Pattern: `once_cell::sync::Lazy` singletons

## Entry Points

**Main Binary:**
- Location: `src/main.rs`
- Triggers: CLI invocation with subcommands (clap)
- Modes: `run` (bot), `refresh-metadata`, `update-ytdlp`, `download` (CLI), `info`
- Responsibilities: Bot initialization, queue processor spawn, background task scheduling

**Library Entry:**
- Location: `src/lib.rs`
- Exports: All public modules for integration tests

**MTProto Binary (experimental):**
- Location: `src/bin/mtproto_download.rs`
- Purpose: Experimental Telegram file download via MTProto

## Error Handling

**Strategy:** Custom error enum with thiserror, category-based tracking

**Patterns:**
- `AppError` enum with variants for each domain (Download, Validation, Database, etc.)
- `AppResult<T>` type alias throughout codebase
- `.category()` method for metrics grouping
- `.track()` method for Prometheus counter increment
- Admin notifications for critical errors - `src/telegram/notifications.rs`
- Error logging to database - `src/core/error_logger.rs`

## Cross-Cutting Concerns

**Logging:**
- `log` crate with `pretty_env_logger` - `src/core/logging.rs`
- Structured logging: `log::info!()`, `log::warn!()`, `log::error!()`
- Contextual emojis for status visibility
- Controlled via `RUST_LOG` environment variable

**Metrics & Monitoring:**
- Prometheus metrics HTTP server - `src/core/metrics_server.rs`
- Custom metrics: download rates, queue depth, error rates - `src/core/metrics.rs`
- Alert manager with Telegram notifications - `src/core/alerts.rs`
- Grafana dashboards - `grafana/provisioning/`

**Rate Limiting:**
- Per-user throttling by subscription plan - `src/core/rate_limiter.rs`
- Configurable limits (free/premium/VIP)
- Auto-cleanup of expired entries

**Background Tasks** (spawned in `src/main.rs`):
- Queue processor (continuous)
- Metrics server (async HTTP)
- Alert monitor (threshold checks)
- Stats reporter (periodic)
- Health check scheduler (smoke tests)
- Subscription expiry checker (hourly)
- Cookie validation (every 5 mins)
- Disk monitoring (every 5 mins)
- Rate limit cleanup (every 5 mins)
- yt-dlp auto-updater (every 6 hours)

---

*Architecture analysis: 2026-02-06*
*Update when major patterns change*
