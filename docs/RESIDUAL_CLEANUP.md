# Residual Cleanup

This file tracks non-blocking cleanup left after the multi-instance rollout work.

## Status

The distributed runtime path is complete.

The items below are not merge blockers for the multi-instance architecture, but they should be cleaned up in follow-up work.

## Remaining Low-Risk Debt

1. `search_cache` is still SQLite-only optimization state.
   - File: `crates/dorabot/src/download/search.rs`
   - Impact: cache quality only, not correctness

2. `metadata_refresh` CLI remains SQLite-oriented.
   - File: `crates/dorabot/src/metadata_refresh.rs`
   - Impact: operational tooling, not runtime correctness

3. `watcher/db.rs` appears legacy and should be audited for deletion or isolation.
   - File: `crates/dorabot/src/watcher/db.rs`

4. `download/playlist_sync/mod.rs` contains low-priority helper code that should be either migrated or removed.
   - File: `crates/dorabot/src/download/playlist_sync/mod.rs`

5. Some helper signatures still carry `DbPool` only for compatibility.
   - Impact: cleanup and readability

## Follow-Up Work

1. Add dedicated Postgres + Redis integration tests.
2. Remove dead SQLite-only helper paths that are no longer used in production.
3. Reduce compatibility parameters where `SharedStorage` is already canonical.
4. Add operational dashboards and lock-ownership visibility.
