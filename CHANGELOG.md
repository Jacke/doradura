# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.31.0] - 2026-03-19

### Added
- Multi-instance runtime with Postgres backend and Redis queue (PR #18)
- `SharedStorage` abstraction — SQLite for dev, Postgres+Redis for production
- `DATABASE_DRIVER` env var to switch between `sqlite` and `postgres`
- Tracing spans with per-task operation IDs for log correlation
- Health monitor crate — auto-recovers bot title, checks `/health`
- Archive ZIP download of user history
- `TempDirGuard` RAII wrapper — eliminates ~40 manual temp file cleanups
- Prometheus `/metrics` endpoint with all download/send/error counters
- Ringtone platform selector (iPhone `.m4r` / Android `.mp3`)

### Changed
- Axum upgraded to 0.8 (path params `{id}` syntax)
- Download module refactored to trait-based `DownloadSource` + `SourceRegistry`

### Fixed
- Axum 0.8 path param syntax (`:id` -> `{id}`) — fixed web server panic
- Tracing subscriber init made non-fatal to prevent crash loops
- Health monitor respects Telegram rate limits, no longer burns `setMyName`
- Archive tables ensured after migration rollback

## [0.30.1] - 2026-03-12

### Fixed
- Dockerfile builder removed from `railway.json`, using GHCR image source
- `set_global_default` + `LogTracer` used separately to avoid log conflict
- `LogTracer::init()` removed — conflicted with tracing-subscriber

## [0.30.0] - 2026-03-10

### Added
- Detailed API logging in health monitor with Retry-After visibility
- URL allowlist enforcement on both preview and download paths

### Fixed
- Health monitor no longer burns `setMyName` rate limit on deploy
- Dependencies updated (quinn-proto CVE, 113 packages)

### Changed
- ~5,400 lines of doracore/dorabot code duplication eliminated

[Unreleased]: https://github.com/Jacke/doradura/compare/v0.31.0...HEAD
[0.31.0]: https://github.com/Jacke/doradura/compare/v0.30.1...v0.31.0
[0.30.1]: https://github.com/Jacke/doradura/compare/v0.30.0...v0.30.1
[0.30.0]: https://github.com/Jacke/doradura/releases/tag/v0.30.0
