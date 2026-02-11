# Code Audit Severity Criteria

Objective, binary criteria for classifying code issues by severity.
These criteria MUST be used by any audit agent analyzing this codebase.

## CRITICAL — Production failure under reachable conditions

An issue is CRITICAL **if and only if** it passes at least one of these 6 binary tests.
If it passes none, it is HIGH or lower. No exceptions.

| # | Test | Question (yes = CRITICAL) |
|---|------|---------------------------|
| 1 | **Panic in production** | Is there an `unwrap()`, `expect()`, `unreachable!()`, `panic!()`, or `[index]` access **outside** `#[test]`/`#[cfg(test)]` where the input **can** be invalid at runtime? |
| 2 | **Hang forever** | Is there a network call, child process, or blocking I/O with **no** timeout and **no** external cancellation mechanism? |
| 3 | **Injection** | Does user-controlled input reach `format!()` for SQL queries or shell commands **without** parameterized binding or allowlist validation? |
| 4 | **Silent data loss** | Is a write to DB or filesystem error silently discarded (`let _ =`) where the caller **cannot** detect the failure? |
| 5 | **Deadlock** | Are 2+ locks acquired in inconsistent order, OR is a `std::sync::Mutex` guard held across an `.await` point? |
| 6 | **Unbounded memory (OOM)** | Does a collection (`Vec`, `HashMap`, `VecDeque`) grow from **external** input with **no** size limit or eviction? |

## What is NOT CRITICAL

These are common false positives that auditors must NOT classify as CRITICAL:

- `unwrap()` / `expect()` inside `#[test]` or `#[cfg(test)]` blocks — standard Rust test practice
- `unwrap()` on infallible operations (e.g., `Regex::new` with a literal, `Client::builder().build()` with safe config)
- `let _ = fs::remove_file()` for temp file cleanup — low impact, not data loss
- Rust RAII file drops — Rust closes file handles automatically on `Drop`
- `Lazy<T>` / `LazyLock<T>` / `OnceLock<T>` initialization — thread-safe by design
- Logging URLs or filenames — not sensitive data unless they contain tokens
- Pattern-matched format strings (e.g., `match quality { "1080p" => ... }`) — not injection
- Hardcoded CLI arguments to `Command::new()` — not injectable
- `tokio::sync::Mutex` held across `.await` — this is exactly what async Mutex is for
- Const arrays that are never empty — `gen_range(0..CONST_ARRAY.len())` is safe
- `u64` arithmetic on disk sizes — overflow is physically impossible

## HIGH — Significant but not immediately catastrophic

Issues that degrade reliability, maintainability, or performance but do not cause immediate production failure:

- Missing error context (`.map_err()` losing original error)
- Blocking sync I/O in async context for **fast local** operations (e.g., `std::fs::metadata` on local files)
- Large files needing modularization (>500 lines)
- Missing input validation at non-critical boundaries
- Swallowed errors on non-essential operations (logging, cleanup)
- Hardcoded values that should be configurable
- Dead code or unused imports

## MEDIUM / LOW

- Code style, naming, documentation
- Minor refactoring opportunities
- Test coverage gaps
- Non-essential TODO comments
