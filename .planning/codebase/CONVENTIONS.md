# Coding Conventions

**Analysis Date:** 2026-02-06

## Naming Patterns

**Files:**
- snake_case for all Rust files: `rate_limiter.rs`, `ytdlp_errors.rs`, `audio_effects.rs`
- `mod.rs` for module directory re-exports
- UPPERCASE.md for project files: `README.md`, `CLAUDE.md`, `CONTRIBUTING.md`

**Functions:**
- snake_case: `download_and_send_video()`, `create_pool()`, `mark_task_completed()`
- Verb-first for actions: `handle_*`, `send_*`, `create_*`, `mark_*`
- Query-style for reads: `get_*`, `is_*`, `has_*`
- No special prefix for async functions

**Variables:**
- snake_case for variables
- SCREAMING_SNAKE_CASE for constants: `MAX_CONCURRENT_DOWNLOADS`, `CHUNK_SIZE`

**Types:**
- PascalCase for structs: `DownloadTask`, `HandlerDeps`, `AppError`
- PascalCase for enums: `TaskPriority`, `BotError`
- No prefix conventions (no `I` for traits)
- Type alias: `pub type AppResult<T> = Result<T, AppError>`

## Code Style

**Formatting:**
- rustfmt with `rustfmt.toml` configuration
- 120 character line width (`max_width = 120`)
- 4-space indentation, no tabs (`hard_tabs = false`)
- Unix newlines (`newline_style = "Unix"`)
- Alphabetical import reordering (`reorder_imports = true`)
- Field init shorthand (`use_field_init_shorthand = true`)
- Match arm pipes never leading

**Linting:**
- clippy with `clippy.toml` configuration
- All warnings treated as errors: `cargo clippy -- -D warnings`
- Cognitive complexity threshold: 30
- Max function arguments: 5
- Type complexity threshold: 250

**Pre-commit hooks:**
- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- Conventional commit message format enforced

## Import Organization

**Order (enforced by rustfmt):**
1. Standard library (`std::*`)
2. External crates (`tokio`, `serde`, `teloxide`)
3. Internal crate modules (`crate::core::*`, `crate::download::*`)

**Grouping:**
- Alphabetical within each group (rustfmt enforced)
- `use` statements at top of file

## Error Handling

**Patterns:**
- `thiserror` for error enum derivation - `src/core/error.rs`
- Custom `AppError` enum with `#[from]` derives for automatic conversions
- `AppResult<T>` type alias used throughout
- `.category()` for metrics grouping, `.track()` for Prometheus increment

**Error Types:**
- Throw on invalid input, external service failures, download errors
- Return `Result<T, AppError>` from all fallible functions
- Log context before returning errors
- Admin notifications for critical failures

## Logging

**Framework:**
- `log` crate (0.4) with `pretty_env_logger` / `simplelog`
- Initialization in `src/core/logging.rs`

**Patterns:**
- `log::info!()`, `log::warn!()`, `log::error!()`, `log::debug!()`
- Contextual emojis for quick visual scanning
- Structured prefixes for grep-ability: `[COOKIE_DIAG:]`, `[BOTH_TIERS_FAILED]`
- Controlled via `RUST_LOG` env var

## Comments

**When to Comment:**
- Module-level doc comments (`//!`) describing purpose at top of each file
- Triple-slash (`///`) for public function documentation
- Explain "why" not "what" for inline comments
- Strategy comments for complex fallback logic (v5.0 download chain)

**Doc Comments:**
- Required for public APIs
- `# Arguments` sections with `*` bullet style
- `# Example` sections with `no_run` where appropriate

**TODO Comments:**
- Format: `// TODO: description`
- Found in: `src/download/audio.rs`, `src/telegram/webapp.rs`, `src/telegram/videos.rs`

## Function Design

**Size:**
- Clippy cognitive complexity limit: 30
- Large functions exist in telegram/ modules (menu.rs, admin.rs) - tech debt

**Parameters:**
- Max 5 parameters (clippy enforced)
- Struct parameter objects for complex signatures (HandlerDeps)

**Return Values:**
- `Result<T, AppError>` for fallible operations
- Early returns for guard clauses
- `Option<T>` for optional values

## Module Design

**Exports:**
- `mod.rs` files declare submodules and re-export public API
- Pattern: `pub use config::*;` and `pub use error::BotError;` in `src/core/mod.rs`
- Internal helpers stay private (not re-exported)

**Configuration:**
- `once_cell::sync::Lazy` for static configuration values
- Environment variables with fallback defaults
- All config centralized in `src/core/config.rs`

## Git Conventions

**Commit Messages:**
- Conventional commits enforced by commit-msg hook
- Types: `feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert`
- Format: `type(optional scope): description` (min 10 chars)
- Example: `fix(smoke+cookies): update to v5.0 strategy (android_vr + deno)`

---

*Convention analysis: 2026-02-06*
*Update when patterns change*
