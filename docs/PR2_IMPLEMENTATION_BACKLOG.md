# PR 2 Implementation Backlog

This branch is the start of the PostgreSQL backend migration that follows PR 1 (`feat-multi-instance`).

## Goal

Add PostgreSQL support for the shared state required by multi-instance operation while preserving SQLite for single-instance deployments.

## Non-goal

Do not rewrite the whole storage layer in one change. The migration must first cover the state that affects correctness across instances.

## Required shared state

1. `processed_updates`
   - webhook send guard
   - durable dedup by `(bot_id, update_id)`

2. `task_queue`
   - claim / lease / heartbeat / retry / dead-letter
   - queue position / idempotency key

3. `users` + `subscriptions`
   - user bootstrap on first message
   - plan / blocked flag / language / download preferences

4. `bot_assets`
   - shared Telegram `file_id` cache across instances

5. session tables
   - `audio_effect_sessions`
   - `audio_cut_sessions`
   - `video_clip_sessions`
   - `cookies_upload_sessions`
   - `ig_cookies_upload_sessions`
   - `lyrics_sessions`

6. vault tables
   - `user_vaults`
   - `vault_cache`

## Optional / follow-up surfaces

1. analytics and admin reporting
2. history export and legacy statistics
3. watcher-specific SQLite helpers
4. backup / restore tooling
5. TUI-specific storage assumptions

## Code migration order

1. Add backend seam for runtime-critical storage.
2. Add PostgreSQL pool and bootstrap schema for required shared tables.
3. Move webhook dedup and queue lifecycle to backend-agnostic calls.
4. Move user bootstrap and user settings reads/writes.
5. Move session and file-id cache tables.
6. Add PostgreSQL integration tests for the required surfaces.
7. Only then port non-critical reporting/admin helpers.

## Files that must change first

- `crates/doracore/src/storage/db/mod.rs`
- `crates/doracore/src/storage/db/sessions.rs`
- `crates/doracore/src/storage/db/vault.rs`
- `crates/dorabot/src/startup.rs`
- `crates/dorabot/src/webhook.rs`
- `crates/dorabot/src/queue_processor.rs`
- `crates/dorabot/src/telegram/handlers/types.rs`
- `crates/doracore/src/i18n.rs`
- `crates/dorabot/src/download/pipeline.rs`
- `crates/dorabot/src/download/audio.rs`
- `crates/dorabot/src/download/video.rs`
- `crates/dorabot/src/telegram/menu/ringtone.rs`
- `crates/dorabot/src/telegram/menu/lyrics.rs`

## Invariants

1. SQLite remains supported for single-instance.
2. PostgreSQL is the canonical backend for multi-instance.
3. Queue state is never in-memory canonical state.
4. Send guard happens before any Telegram side effect.
5. A task can be leased by only one live worker at a time.
6. Runtime-critical state cannot depend on a local SQLite file in multi-instance mode.

## Immediate next implementation slice

Implement a backend abstraction for:
- `register_processed_update`
- `cleanup_old_processed_updates`
- queue lifecycle functions
- `get_user` / `create_user` / `create_user_with_language`
- user preference reads used by the download pipeline
- `get_bot_asset` / `set_bot_asset`
- session CRUD used by ringtone / audio effects / cookies flows
