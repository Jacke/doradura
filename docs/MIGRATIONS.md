# Database migrations (refinery + SQLite)

We now use [refinery](https://github.com/rust-db/refinery) for versioned SQL migrations. All migrations live in `migrations/` and are applied automatically on startup.

## Adding a migration
1. Create a new SQL file in `migrations/` with the next version, e.g. `V3__my_change.sql`.
2. Put **pure SQL** inside (no Rust required). The file name must follow `V{number}__description.sql`.
3. Keep migrations idempotent for SQLite (use `IF NOT EXISTS` where possible).

## Running migrations locally
- Migrations run automatically when the bot starts.
- To run them manually, install the refinery CLI (optional):  
  `cargo install refinery_cli`  
  Then run:  
  `refinery migrate -e sqlite -p database.sqlite`

## Current baseline
- `V1__initial_schema.sql` – current schema without the language column.
- `V2__add_language.sql` – adds `language` to `users` with default `ru`.

When you add new columns/tables, create a new versioned SQL file instead of editing previous ones. README/CI do not need changes: the runtime migration step keeps the DB up to date.
