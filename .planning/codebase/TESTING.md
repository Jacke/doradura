# Testing Patterns

**Analysis Date:** 2026-02-06

## Test Framework

**Runner:**
- Standard Rust test framework (`#[test]`, `#[tokio::test]`)
- Criterion 0.5 for benchmarks - `benches/queue_benchmark.rs`

**Assertion Library:**
- Standard `assert!()`, `assert_eq!()`, `assert_ne!()`
- `#[should_panic]` for expected panics

**Run Commands:**
```bash
cargo test                                    # Run all tests
cargo test --test smoke_test -- --nocapture   # Smoke tests with output
cargo test test_name                          # Single test
cargo bench --bench queue_benchmark           # Benchmarks
```

## Test File Organization

**Location:**
- Unit tests: `#[cfg(test)]` modules inline in source files
- Integration tests: `tests/*.rs`
- Shared utilities: `tests/common/`
- Mocks: `tests/mocks/`
- Benchmarks: `benches/`

**Naming:**
- Integration tests: `*_test.rs` or `*_integration_test.rs`
- Test functions: `test_*` prefix

**Structure:**
```
src/
  core/
    error.rs          # 17 unit tests inline
    logging.rs        # 2 unit tests inline
    rate_limiter.rs   # Unit tests inline
  download/
    cookies.rs        # Unit tests inline
tests/
  smoke_test.rs              # Real YouTube download tests
  core_modules_test.rs       # Core module integration tests
  bot_integration_test.rs    # Bot handler tests
  ytdlp_integration_test.rs  # yt-dlp integration
  proxy_integration_test.rs  # Proxy chain tests
  handlers_integration_test.rs
  e2e_test.rs               # End-to-end workflows
  bot_snapshots_test.rs     # Snapshot-based tests
  load_test.rs              # Load testing
  common/
    fixtures.rs              # Test data factories
    helpers.rs               # Common utilities
    recorder.rs              # Request/response recording
    snapshots.rs             # Snapshot support
  mocks/                     # Mock implementations
benches/
  queue_benchmark.rs         # Queue performance benchmarks
```

## Test Structure

**Suite Organization:**
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_case() {
        // arrange
        let input = create_test_input();
        // act
        let result = function_under_test(input);
        // assert
        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_async_operation() {
        let result = async_function().await;
        assert!(result.is_ok());
    }
}
```

**Patterns:**
- `#[cfg(test)]` module at end of source file
- `use super::*;` for access to parent module
- `#[tokio::test]` for async tests
- `serial_test` crate for tests requiring isolation

## Mocking

**Framework:**
- wiremock 0.5 for HTTP mocking - `Cargo.toml`
- fake 2.9 for test data generation - `Cargo.toml`
- Custom mock implementations in `tests/mocks/`

**Patterns:**
```rust
// HTTP mocking with wiremock
let mock_server = MockServer::start().await;
Mock::given(method("GET"))
    .and(path("/api/endpoint"))
    .respond_with(ResponseTemplate::new(200).set_body_json(json!({"key": "value"})))
    .mount(&mock_server)
    .await;
```

**What to Mock:**
- External HTTP APIs (YouTube, Telegram)
- File system operations in unit tests
- Time-dependent operations

**What NOT to Mock:**
- Internal pure functions
- Database operations (use real SQLite in tests)

## Fixtures and Factories

**Test Data:**
- Factory functions in `tests/common/fixtures.rs`
- Inline test data for simple cases
- `fake` crate for generated data

**Location:**
- `tests/common/fixtures.rs` - Shared factories
- `tests/common/helpers.rs` - Common test utilities
- Inline in test file for module-specific data

## Coverage

**Requirements:**
- No enforced coverage target
- Focus on critical paths (download pipeline, error handling)

**Configuration:**
- No coverage tooling configured in CI
- Manual via `cargo tarpaulin` or similar

## Test Types

**Unit Tests:**
- Inline `#[cfg(test)]` modules
- Test individual functions in isolation
- Located: `src/core/error.rs` (17 tests), `src/core/logging.rs`, `src/download/cookies.rs`

**Integration Tests:**
- `tests/` directory
- Test cross-module interactions
- Use real database (tempfile SQLite)
- `tests/core_modules_test.rs`, `tests/bot_integration_test.rs`

**Smoke Tests:**
- Real-world YouTube downloads
- Default URL: first YouTube video (~19 seconds)
- Tests: ffmpeg, ffprobe, cookies validation, metadata, audio/video download
- `tests/smoke_test.rs` + `src/smoke_tests/`

**Load Tests:**
- Custom harness for capacity testing
- `tests/load_test.rs`

**Benchmarks:**
- Criterion-based queue performance benchmarks
- `benches/queue_benchmark.rs`

## CI/CD Integration

**GitHub Actions** (`.github/workflows/ci.yml`):
```bash
cargo fmt --all -- --check          # Formatting
cargo clippy -- -D warnings         # Linting
cargo test --verbose                # All tests
cargo test --test smoke_test        # Smoke tests
```

**Pre-commit Hooks:**
- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- Conventional commit message validation

## Common Patterns

**Async Testing:**
```rust
#[tokio::test]
async fn test_async_download() {
    let result = download_metadata("url").await;
    assert!(result.is_ok());
}
```

**Error Testing:**
```rust
#[test]
fn test_error_conversion() {
    let err = AppError::from("test error".to_string());
    assert_eq!(err.category(), ErrorCategory::Download);
}
```

**Snapshot Testing:**
- Supported via `tests/common/snapshots.rs`
- Used in `tests/bot_snapshots_test.rs`

---

*Testing analysis: 2026-02-06*
*Update when test patterns change*
