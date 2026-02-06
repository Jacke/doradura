# Technology Stack

**Analysis Date:** 2026-02-06

## Languages

**Primary:**
- Rust 2021 edition - All application code (`Cargo.toml`)

**Secondary:**
- Python 3 - yt-dlp runtime, cookie extraction (`tools/cookie_manager.py`)
- JavaScript/Node.js - bgutil PO token server (`Dockerfile.s6`)
- Shell scripts - Docker entrypoints, test helpers

## Runtime

**Environment:**
- Rust 1.85+ (musl target for Alpine Linux) - `Dockerfile.s6`
- Tokio 1.40 async runtime (full features) - `Cargo.toml`
- Alpine Linux (production container)
- Node.js 22+ (yt-dlp JavaScript runtime) - `Dockerfile.s6`
- Deno (yt-dlp YouTube n-challenge solving, Alpine edge/testing) - `Dockerfile.s6`
- Python 3 (yt-dlp, cookie management) - `Dockerfile.s6`
- FFmpeg (audio/video conversion) - `Dockerfile.s6`

**Package Manager:**
- Cargo (Rust) - `Cargo.lock` present
- pip3 (Python packages) - `Dockerfile.s6`
- npm (bgutil PO token server) - `Dockerfile.s6`

## Frameworks

**Core:**
- Teloxide 0.17 - Telegram Bot API framework with macros - `Cargo.toml`
- Axum 0.7 - HTTP web framework (webapp, metrics server) - `Cargo.toml`
- Tonic 0.10 - gRPC framework (downsub integration) - `Cargo.toml`
- Tower 0.4 / Tower-HTTP 0.5 - HTTP middleware (CORS, file serving) - `Cargo.toml`

**Testing:**
- Standard Rust `#[test]` and `#[tokio::test]`
- Criterion 0.5 - Benchmarking - `benches/queue_benchmark.rs`
- wiremock 0.5 - HTTP mocking - `Cargo.toml`
- fake 2.9 - Test data generation - `Cargo.toml`
- serial_test 3.0 - Test isolation - `Cargo.toml`

**Build/Dev:**
- cargo fmt (rustfmt) - `rustfmt.toml`
- cargo clippy - `clippy.toml`
- Docker multi-stage builds with BuildKit cache mounts - `Dockerfile.s6`
- s6-overlay v3.2.0.2 - Process supervision - `Dockerfile.s6`

## Key Dependencies

**Critical:**
- `teloxide 0.17` - Telegram bot framework (core functionality) - `Cargo.toml`
- `rusqlite 0.32` + `r2d2 0.8` - SQLite database with connection pooling - `Cargo.toml`
- `reqwest 0.12` - HTTP client (API calls, downloads) - `Cargo.toml`
- `serde 1.0` + `serde_json 1.0` - Serialization - `Cargo.toml`
- `tokio 1.40` - Async runtime - `Cargo.toml`
- `prometheus 0.14` - Metrics collection - `Cargo.toml`

**Infrastructure:**
- `refinery 0.8` - Database migrations - `Cargo.toml`
- `dotenvy 0.15` - Environment file loading - `Cargo.toml`
- `lazy_static 1.4` + `once_cell 1.19` - Static initialization - `Cargo.toml`
- `thiserror 1.0` + `anyhow 1.0` - Error handling - `Cargo.toml`
- `log 0.4` + `pretty_env_logger 0.5` - Logging - `Cargo.toml`
- `fluent-templates 0.8` - Internationalization - `Cargo.toml`
- `grammers-client 0.6` - MTProto (experimental) - `Cargo.toml`
- `hmac 0.12` + `sha2 0.10` - HMAC authentication - `Cargo.toml`
- `prost 0.12` - Protocol Buffers (gRPC) - `Cargo.toml`

## Configuration

**Environment:**
- `.env` files via dotenvy (`.env`, `.env.example`, `.env.staging`)
- `once_cell::sync::Lazy` statics loaded from env vars at startup - `src/core/config.rs`
- Key vars: `TELOXIDE_TOKEN`, `DATABASE_PATH`, `YTDL_BIN`, `WARP_PROXY`, `ADMIN_IDS`

**Build:**
- `Cargo.toml` - Package manifest
- `.cargo/config.toml` - Build environment setup
- `build.rs` - Build script (protobuf generation)
- `rustfmt.toml` - Formatting (120 char line width)
- `clippy.toml` - Linting (cognitive complexity: 30, max args: 5)

## Platform Requirements

**Development:**
- macOS/Linux (any platform with Rust toolchain)
- FFmpeg + ffprobe required for media operations
- yt-dlp binary required for downloads
- SQLite3

**Production:**
- Railway platform - `railway.toml`
- Docker container (Alpine Linux + s6-overlay)
- External services: Telegram Bot API, YouTube (via yt-dlp)
- Optional: Local Telegram Bot API server - `docker-compose.bot-api.yml`

---

*Stack analysis: 2026-02-06*
*Update after major dependency changes*
