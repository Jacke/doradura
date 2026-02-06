# Codebase Structure

**Analysis Date:** 2026-02-06

## Directory Layout

```
doradura/
├── src/                        # Rust application source
│   ├── main.rs                 # Entry point, bot setup, queue processor
│   ├── lib.rs                  # Library manifest
│   ├── cli.rs                  # CLI argument parsing (clap)
│   ├── i18n.rs                 # Internationalization
│   ├── downsub.rs              # Downsub gRPC integration
│   ├── metadata_refresh.rs     # Batch metadata refresh
│   ├── bin/                    # Additional binaries
│   ├── core/                   # Infrastructure & utilities
│   ├── storage/                # Data persistence (SQLite)
│   ├── download/               # Download orchestration
│   ├── telegram/               # Bot & UI layer
│   ├── conversion/             # Media format conversion
│   ├── timestamps/             # Video timestamp parsing
│   ├── smoke_tests/            # Health checks
│   ├── testing/                # Test utilities
│   └── experimental/           # Experimental features (MTProto)
├── tests/                      # Integration tests
│   ├── common/                 # Shared test utilities
│   ├── mocks/                  # Mock implementations
│   └── *.rs                    # Integration test files
├── benches/                    # Criterion benchmarks
├── migrations/                 # SQL database migrations (V1-V22)
├── locales/                    # i18n fluent templates
├── tools/                      # Python utilities
│   ├── cookie_manager.py       # Headless browser cookie extraction
│   └── log_to_snapshot.py      # Log snapshot utility
├── grafana/                    # Grafana dashboard provisioning
├── prometheus/                 # Prometheus configuration
├── rootfs/                     # s6-overlay service definitions
├── .github/workflows/          # CI/CD pipelines
├── Cargo.toml                  # Rust package manifest
├── Dockerfile.s6               # Production Docker image
├── docker-compose.bot-api.yml  # Local Bot API server
├── railway.toml                # Railway deployment config
└── build.rs                    # Build script (protobuf)
```

## Directory Purposes

**src/core/**
- Purpose: Foundational infrastructure used by all other modules
- Contains: Config, error types, rate limiter, metrics, alerts, stats, logging, subscriptions
- Key files: `config.rs` (env config), `error.rs` (AppError), `metrics.rs` (Prometheus)
- All other modules depend on this

**src/storage/**
- Purpose: Data persistence and caching
- Contains: SQLite operations (r2d2 pool), migrations, cache, backup
- Key files: `db.rs` (all DB operations), `migrations.rs` (refinery)

**src/download/**
- Purpose: Download orchestration and media processing
- Contains: Video/audio download, queue, cookies, yt-dlp wrapper, proxy, metadata
- Key files: `video.rs` (1595 lines), `audio.rs` (1192 lines), `queue.rs` (738 lines), `cookies.rs` (1574 lines)
- Largest module by code volume

**src/telegram/**
- Purpose: Bot interface, handlers, UI, admin tools
- Contains: Handler tree, commands, menus, admin, analytics, preview, webapp
- Key files: `handlers.rs` (handler schema), `menu.rs` (3855 lines), `admin.rs` (3294 lines), `commands.rs` (2940 lines)
- Largest module by file count

**src/conversion/**
- Purpose: Media format conversion
- Contains: Video processing (video notes, compression), image resizing, DOCX-to-PDF
- Key files: `video.rs`, `image.rs`, `document.rs`

**src/timestamps/**
- Purpose: Video timestamp and chapter parsing
- Contains: URL parser, extractor, chapter/description parsers
- Key files: `url_parser.rs`, `extractor.rs`, `chapter_parser.rs`

**src/smoke_tests/**
- Purpose: Health checks and monitoring
- Contains: Test cases, runner, validators, scheduler, results
- Key files: `test_cases.rs`, `runner.rs`, `scheduler.rs`

**migrations/**
- Purpose: SQL database schema migrations
- Contains: V1 through V22 SQL files
- Pattern: `V{N}__{description}.sql`

**rootfs/**
- Purpose: s6-overlay service definitions for Docker
- Contains: Service run scripts, finish scripts, dependencies

## Key File Locations

**Entry Points:**
- `src/main.rs` - Main binary entry (1374 lines)
- `src/lib.rs` - Library manifest
- `src/cli.rs` - CLI argument definitions (clap)
- `src/bin/mtproto_download.rs` - Experimental MTProto binary

**Configuration:**
- `Cargo.toml` - Rust dependencies and build config
- `.cargo/config.toml` - Build environment
- `rustfmt.toml` - Code formatting (120 char width)
- `clippy.toml` - Linting thresholds
- `.env.example` - Environment variable template
- `railway.toml` - Railway deployment config

**Core Logic:**
- `src/download/video.rs` - Video download pipeline
- `src/download/audio.rs` - Audio download pipeline
- `src/download/queue.rs` - Priority download queue
- `src/download/cookies.rs` - Cookie management
- `src/download/metadata.rs` - Metadata extraction
- `src/telegram/handlers.rs` - Handler dispatch tree

**Testing:**
- `tests/smoke_test.rs` - Smoke test suite
- `tests/core_modules_test.rs` - Core module tests
- `tests/common/` - Shared test utilities
- `benches/queue_benchmark.rs` - Queue benchmarks
- `tests/README.md` - Test documentation

**Documentation:**
- `README.md` - Project overview
- `CONTRIBUTING.md` - Contribution guidelines
- `CLAUDE.md` - Claude Code instructions

## Naming Conventions

**Files:**
- snake_case for all Rust source: `rate_limiter.rs`, `ytdlp_errors.rs`
- `mod.rs` for module directory exports
- `V{N}__{description}.sql` for migrations

**Directories:**
- snake_case: `smoke_tests/`, `audio_effects/`
- Plural for collections: `tests/`, `migrations/`, `locales/`

**Special Patterns:**
- `mod.rs` declares submodules and re-exports public API
- Test modules inline with `#[cfg(test)]`
- Integration tests in `tests/` directory

## Where to Add New Code

**New Download Feature:**
- Primary code: `src/download/`
- Tests: `#[cfg(test)]` module in same file + `tests/`
- Config if needed: `src/core/config.rs`

**New Telegram Command:**
- Handler: `src/telegram/commands.rs` or new file in `src/telegram/`
- Register in: `src/telegram/handlers.rs`
- Tests: `tests/handlers_integration_test.rs`

**New Background Task:**
- Spawn in: `src/main.rs`
- Logic in: appropriate module (`src/core/`, `src/download/`)

**New Database Table:**
- Migration: `migrations/V{N+1}__description.sql`
- Operations: `src/storage/db.rs`

**Utilities:**
- Shared helpers: `src/core/utils.rs`
- Type definitions: `src/telegram/types.rs` or module-specific

## Special Directories

**target/**
- Purpose: Rust build artifacts
- Source: Generated by cargo
- Committed: No (in .gitignore)

**rootfs/**
- Purpose: s6-overlay service definitions for Docker container
- Source: Hand-written service scripts
- Committed: Yes

**grafana/ + prometheus/**
- Purpose: Monitoring stack configuration
- Source: Hand-written dashboards and alert rules
- Committed: Yes

---

*Structure analysis: 2026-02-06*
*Update when directory structure changes*
