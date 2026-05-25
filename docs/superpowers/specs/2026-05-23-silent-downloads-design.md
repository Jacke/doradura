# Silent Downloads + MOTD digest — design

## Goal

A "silent downloads" mode: requested downloads are processed at **low priority
("when there's time")**, produce **no message flood** (no queue-position
message, no progress edits, no notification ping on delivery), and are recapped
to the user as a **MOTD digest on their next interaction** with the bot.

## Decisions (locked with user)

1. **Trigger:** a persistent per-user flag, flipped both from a Settings toggle
   and from a button on the preview card. Both control the *same* flag (simpler
   than per-download state, and avoids threading a `silent` column through the
   dual-backend `task_queue` persistence).
2. **Delivery:** the finished file is sent **silently right away**
   (`disable_notification`, no progress spam). A MOTD recap appears on the next
   interaction. (Not "hold everything until next time".)
3. **Priority:** silent tasks run at `TaskPriority::Low` — normal downloads
   always overtake them.
4. **Acknowledgement:** in silent mode the bot reacts 👌 on the user's link
   message instead of sending a queue-position message.
5. **Failures** are also recorded in the digest (`status = 'failed'`).

## Components

### Storage (doracore)
- **Migration `V49__silent_downloads.sql`** (SQLite/refinery): `ALTER TABLE
  users ADD COLUMN silent_downloads INTEGER DEFAULT 0;` + `CREATE TABLE
  silent_digest(id, user_id, title, format, status, completed_at, shown)`.
- Mirror in the two idempotent bootstraps: `migrations.rs` ensure-block (SQLite)
  and `pg_bootstrap.rs` (Postgres `DO $$ … duplicate_column` + `CREATE TABLE IF
  NOT EXISTS`).
- `user_settings.rs`: `get_user_silent_downloads` / `set_user_silent_downloads`
  (existing i32-setting helper pattern).
- New `shared/silent_digest.rs` + `db/silent_digest.rs`:
  - `insert_silent_digest(user_id, title, format, status)`
  - `take_unshown_silent_digest(user_id) -> Vec<SilentDigestEntry>` — fetch
    `shown = 0` rows and mark them `shown = 1` atomically (idempotent: a second
    rapid interaction gets nothing, so no double MOTD).
  - `prune_shown_silent_digest(older_than)` — periodic cleanup.

### Bot (dorabot)
- `DownloadContext` gains `silent: bool`.
- **Enqueue** (`menu/callback_download.rs`): read `get_user_silent_downloads`;
  if silent → `priority = Low`, skip the queue-position message, react 👌.
- **Worker** (`queue_processor.rs`): read the user's silent flag when building
  `DownloadContext`, set `ctx.silent`.
- **Suppression:** `ProgressMessage` gets a silent constructor (no status
  message; `update()` is a no-op). The final `send_video`/`send_audio` adds
  `.disable_notification(true)` when `ctx.silent`.
- **Digest write:** at the point the download is saved to `download_history`
  (title + format known), if `ctx.silent` insert a `silent_digest` row
  (`done`); failure paths insert `failed`.
- **MOTD on next interaction:** helper `maybe_show_silent_digest(bot, storage,
  user_id)` called at the **top of the message and callback endpoints** in
  `handlers/schema.rs` (a plain call, not a dptree branch, so it never consumes
  or blocks normal routing). Sends one digest, marks rows shown.
- **Settings toggle:** button in the settings menu → callback
  `settings:silent:toggle`.
- **Preview button:** chip on the preview card → callback `pv:silent:{url_id}`
  that flips the same user flag and re-renders the card.

## Data flow

```
link → preview (🔇 chip reflects flag)
     → dl:* press → enqueue Low + (silent? react 👌 : queue-pos msg)
     → worker picks up when free → ctx.silent ⇒ no progress msgs
     → file sent disable_notification → insert silent_digest('done')
     → user's next msg/callback → MOTD digest → mark rows shown
```

## Testing
- Unit: setting getter/setter round-trip; `take_unshown_silent_digest`
  idempotency (second call returns empty); MOTD text builder formatting;
  preview-flag reflected in card keyboard.
- The ffmpeg/Telegram send path is otherwise unchanged → no new integration risk.

## Out of scope (YAGNI)
- Per-download silent state distinct from the global flag.
- Holding files until next interaction / a storage channel.
- i18n for the new strings in this first cut (hardcoded RU, matching the
  surrounding `downloads/*` modules); can be localised later.
