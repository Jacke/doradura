# CLI Implementation Summary

## What Was Done

A full CLI (Command Line Interface) system was implemented for the bot with support for multiple run modes and utility commands.

## New Files

### 1. `src/cli.rs` - CLI Structure

Defines the CLI structure using the `clap` library:

```rust
pub enum Commands {
    Run { webhook: bool },
    RunStaging { webhook: bool },
    RunWithCookies { cookies: Option<String>, webhook: bool },
    RefreshMetadata { limit: Option<usize>, dry_run: bool, verbose: bool },
}
```

**Features:**
- Automatic help generation (`--help`)
- Type-safe arguments
- Subcommands with options
- Version flag (`--version`)

### 2. `src/metadata_refresh.rs` - Metadata Refresh Utility

Utility for updating missing metadata in the `download_history` table.

**Functionality:**
- Finds records with `file_id` but no metadata
- Downloads the file from Telegram
- Extracts metadata using `ffprobe`:
  - `file_size` - file size
  - `duration` - duration
  - `video_quality` - video resolution (for mp4)
  - `audio_bitrate` - audio bitrate (for mp3)
- Updates the database
- Supports `--dry-run` for safe testing
- Supports `--limit` to cap the number of processed records
- Supports `--verbose` for detailed output

**Main functions:**
- `refresh_missing_metadata()` - main entry point
- `download_telegram_file()` - downloads a file by file_id
- `extract_metadata()` - extracts metadata via ffprobe
- `update_metadata()` - updates the database record

### 3. `CLI_USAGE.md` - Documentation

Complete documentation on CLI usage:
- Description of all commands
- Usage examples
- Troubleshooting
- Migration from shell scripts

## Modified Files

### 1. `Cargo.toml`

Added `clap` dependency:

```toml
clap = { version = "4.5", features = ["derive", "cargo"] }
```

### 2. `src/lib.rs`

Added new modules:

```rust
pub mod cli;
pub mod metadata_refresh;
```

### 3. `src/main.rs`

Fully refactored to support CLI:

**Changes:**
- Added command-line argument parsing
- Created `run_bot(use_webhook: bool)` function - all bot startup code moved here
- Created `run_metadata_refresh()` function - launches the metadata refresh utility
- `main()` is now a command dispatcher:
  ```rust
  match cli.command {
      Some(Commands::Run { webhook }) => run_bot(webhook).await,
      Some(Commands::RunStaging { webhook }) => { /* load .env.staging */ run_bot(webhook).await },
      Some(Commands::RunWithCookies { cookies, webhook }) => { /* set cookies */ run_bot(webhook).await },
      Some(Commands::RefreshMetadata { ... }) => run_metadata_refresh(...).await,
      None => run_bot(false).await,  // default
  }
  ```

**Webhook support:**
- Added `use_webhook` parameter to `run_bot()`
- Webhook is enabled only if the parameter is `true` AND `WEBHOOK_URL` is set

### 4. `README.md`

Added link to CLI documentation:

```markdown
> **Note:** The bot now supports CLI commands. See [CLI_USAGE.md](CLI_USAGE.md)
> for all available commands including `run-staging`, `run-with-cookies`, and `refresh-metadata`.
```

## Available Commands

### 1. `doradura run [--webhook]`

Runs the bot in normal mode (uses `.env`).

**Examples:**
```bash
./doradura run
./doradura run --webhook
```

### 2. `doradura run-staging [--webhook]`

Runs the bot in staging mode (uses `.env.staging`).

**Examples:**
```bash
./doradura run-staging
./doradura run-staging --webhook
```

### 3. `doradura run-with-cookies [--cookies PATH] [--webhook]`

Runs the bot with a specified cookies file path.

**Examples:**
```bash
./doradura run-with-cookies
./doradura run-with-cookies --cookies /path/to/cookies.txt
./doradura run-with-cookies --cookies cookies.txt --webhook
```

### 4. `doradura refresh-metadata [OPTIONS]`

Updates missing metadata in download_history.

**Options:**
- `-l, --limit <N>` - Process only the first N records
- `--dry-run` - Show what would be updated without applying changes
- `-v, --verbose` - Verbose output

**Examples:**
```bash
# Dry run
./doradura refresh-metadata --dry-run --verbose

# Update first 10
./doradura refresh-metadata --limit 10

# Update all with verbose output
./doradura refresh-metadata --verbose

# Update all (silent)
./doradura refresh-metadata
```

## Advantages

### 1. Single Entry Point

**Before:**
- `run_staging.sh`
- `run_with_cookies.sh`
- Different scripts for different tasks

**After:**
```bash
./doradura <command>
```

### 2. Built-in Documentation

```bash
./doradura --help
./doradura refresh-metadata --help
```

### 3. Type Safety

Clap validates arguments at parse time:
- `--limit` must be a number
- `--cookies` accepts a string
- Flags (`--webhook`, `--dry-run`, `--verbose`) are booleans

### 4. Extensibility

Easy to add new commands:

```rust
// In src/cli.rs
pub enum Commands {
    // ...
    Backup { output: Option<String> },
    Stats,
    Clean,
}

// In src/main.rs
match cli.command {
    // ...
    Some(Commands::Backup { output }) => run_backup(output).await,
    Some(Commands::Stats) => run_stats().await,
    Some(Commands::Clean) => run_clean().await,
}
```

## Use Cases

### Development

```bash
# Run in dev mode
cargo run -- run

# Staging with a different database
cargo run -- run-staging

# Test metadata refresh
cargo run -- refresh-metadata --dry-run --limit 5
```

### Production

```bash
# Build
cargo build --release

# Run
./target/release/doradura run

# Systemd service
[Service]
ExecStart=/opt/doradura/doradura run
```

### Maintenance

```bash
# Refresh metadata after migration
./doradura refresh-metadata

# Run with new cookies
./doradura run-with-cookies --cookies fresh_cookies.txt
```

## Migration

### Before (Shell Scripts)

**run_staging.sh:**
```bash
#!/bin/bash
export $(cat .env.staging | xargs)
cargo run
```

**run_with_cookies.sh:**
```bash
#!/bin/bash
export YOUTUBE_COOKIES_PATH=/path/to/cookies.txt
cargo run
```

### After (CLI)

```bash
# Just commands
./doradura run-staging
./doradura run-with-cookies --cookies /path/to/cookies.txt
```

## Testing

### Build

```bash
cargo build
# Successful compilation
```

### Run help

```bash
./target/debug/doradura --help
# Shows all commands

./target/debug/doradura refresh-metadata --help
# Shows refresh-metadata options
```

### Testing commands

```bash
# Run (default)
./doradura
# Starts bot in default mode

# Refresh metadata (dry run)
./doradura refresh-metadata --dry-run
# Shows entries to refresh without making changes
```

## Dependencies

### New

- `clap = "4.5"` - CLI argument parsing

### Used in metadata_refresh

- `reqwest` - HTTP requests for downloading files from Telegram (already present)
- `serde_json` - JSON response parsing from Telegram API (already present)
- `uuid` - Unique temporary filename generation (already present)
- `ffprobe` - System utility for metadata extraction (requires installation)

## Requirements

### Runtime

- `ffprobe` must be installed for `refresh-metadata`:
  ```bash
  # macOS
  brew install ffmpeg

  # Ubuntu/Debian
  sudo apt-get install ffmpeg
  ```

### Environment Variables

All commands require a `.env` file with:
- `BOT_TOKEN` - for all commands
- `WEBHOOK_URL` - only for `--webhook` mode
- Other variables from `config.rs`

## Roadmap

Planned commands:

1. `doradura backup [--output PATH]` - Create a database backup
2. `doradura stats` - Usage statistics
3. `doradura migrate` - Run migrations
4. `doradura clean` - Clean up temporary files
5. `doradura export [--format csv|json]` - Export data
6. `doradura validate` - Validate configuration

## Breaking Changes

### For Railway/Docker

The startup command needs to be updated:

**Docker:**
```dockerfile
# Before
CMD ["./doradura"]

# After
CMD ["./doradura", "run"]
```

**Railway:**
```
Start Command: ./doradura run
```

### For Systemd

```ini
[Service]
# Before
ExecStart=/opt/doradura/doradura

# After
ExecStart=/opt/doradura/doradura run
```

**Backward compatibility:**
Running without arguments (`./doradura`) still works - starts the bot in default `run` mode.

## Files

### Created

1. `src/cli.rs` - CLI structure (59 lines)
2. `src/metadata_refresh.rs` - Metadata refresh utility (282 lines)
3. `CLI_USAGE.md` - Documentation (400+ lines)
4. `CLI_IMPLEMENTATION_SUMMARY.md` - This file

### Modified

1. `Cargo.toml` - Added clap
2. `src/lib.rs` - Export new modules
3. `src/main.rs` - CLI refactoring (~100 lines changed)
4. `README.md` - Link to CLI documentation

## Summary

Implemented:
- CLI system with 4 commands
- Metadata refresh utility
- Staging environment support
- Cookies support via arguments
- Webhook toggle via flag
- Complete documentation

Quality:
- Type-safe arguments
- Built-in help text
- Dry-run mode for safety
- Verbose mode for debugging
- Backward compatible

Ready to use:
- Compiles without errors
- Tested `--help`
- Documentation complete
- Usage examples provided

## How to Use

1. **Build:**
   ```bash
   cargo build --release
   ```

2. **Run the bot:**
   ```bash
   ./target/release/doradura run
   ```

3. **Refresh metadata:**
   ```bash
   ./target/release/doradura refresh-metadata --dry-run --verbose
   ```

4. **Help:**
   ```bash
   ./target/release/doradura --help
   ```
