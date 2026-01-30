# Snapshot Testing - Complete Telegram Bot Testing System

System for recording real Telegram API interactions and replaying them in tests.

## What This Provides

- **Capture real bot interactions** - all responses are taken from real API calls
- **Fast tests** - no real network requests
- **Deterministic** - tests always give the same result
- **Offline work** - can test without internet
- **Documentation** - snapshots show how the API works

## Current State

- **Snapshots**: 7
- **API methods**: 7 different (sendMessage, sendPhoto, sendAudio, ...)
- **Tests**: 18 automatic tests
- **Scenarios**: Commands, settings, downloads, errors

## Quick Start

### 1. View existing snapshots

```bash
ls tests/snapshots/*.json
```

Available:
- `start_command.json` - /start command
- `info_command.json` - Format information
- `settings_menu.json` - Settings menu
- `youtube_processing.json` - YouTube URL processing
- `audio_download_complete.json` - Complete download cycle
- `language_selection.json` - Language selection
- `rate_limit_error.json` - Rate limit error

### 2. Run tests

```bash
# All snapshot tests
cargo test --test bot_snapshots_test --test bot_commands_test

# Specific test
cargo test test_youtube_processing_flow

# With output
cargo test --test bot_commands_test -- --nocapture
```

### 3. Use in your tests

```rust
use common::TelegramMock;

#[tokio::test]
async fn test_my_feature() {
    let mock = TelegramMock::from_snapshot("youtube_processing").await?;
    let bot = mock.create_bot()?;

    // Use bot - all responses will be from snapshot
    // handle_youtube_url(&bot, url).await?;
}
```

## Project Structure

```
doradura/
├── src/
│   └── testing/              # (for unit tests only)
│       ├── mod.rs
│       ├── snapshots.rs
│       └── recorder.rs
│
├── tests/
│   ├── common/               # Shared testing utilities
│   │   ├── mod.rs
│   │   ├── snapshots.rs      # TelegramMock, TelegramSnapshot
│   │   └── recorder.rs       # RecordingClient (helper)
│   │
│   ├── snapshots/            # JSON snapshots
│   │   ├── README.md
│   │   ├── SNAPSHOT_INDEX.md
│   │   ├── start_command.json
│   │   ├── info_command.json
│   │   ├── settings_menu.json
│   │   ├── language_selection.json
│   │   ├── youtube_processing.json
│   │   ├── audio_download_complete.json
│   │   └── rate_limit_error.json
│   │
│   ├── bot_snapshots_test.rs    # Basic tests
│   └── bot_commands_test.rs     # Detailed command tests
│
├── tools/
│   └── log_to_snapshot.py    # Log to JSON converter
│
└── docs/
    ├── SNAPSHOT_TESTING.md           # Full documentation
    └── SNAPSHOT_TESTING_QUICKSTART.md # Quick start
```

## How to Create a New Snapshot

### Method 1: Manual (recommended)

1. Run the bot with logging:
   ```bash
   RUST_LOG=debug cargo run
   ```

2. Perform the action in Telegram (e.g., send /info)

3. Copy JSON from logs

4. Create file `tests/snapshots/my_test.json`:
   ```json
   {
     "name": "my_test",
     "version": "1.0",
     "recorded_at": "2026-01-04T12:00:00Z",
     "interactions": [
       [
         {"method": "POST", "path": "/sendMessage", "body": {...}, "timestamp": 123},
         {"status": 200, "body": {...}, "headers": {...}}
       ]
     ],
     "metadata": {}
   }
   ```

### Method 2: Python utility

```bash
# Interactive mode
./tools/log_to_snapshot.py --interactive

# From log file
./tools/log_to_snapshot.py --input bot.log --name my_test

# From stream
cargo run 2>&1 | ./tools/log_to_snapshot.py --stdin --name my_test
```

## Documentation

- **[SNAPSHOT_TESTING.md](docs/SNAPSHOT_TESTING.md)** - Full guide (200+ lines)
- **[SNAPSHOT_TESTING_QUICKSTART.md](docs/SNAPSHOT_TESTING_QUICKSTART.md)** - Quick start
- **[tests/snapshots/README.md](tests/snapshots/README.md)** - List of all snapshots
- **[tests/snapshots/SNAPSHOT_INDEX.md](tests/snapshots/SNAPSHOT_INDEX.md)** - Index with details

## Test Examples

### Basic command test
```rust
#[tokio::test]
async fn test_info_command() {
    let mock = TelegramMock::from_snapshot("info_command").await?;
    let snapshot = mock.snapshot();

    assert_eq!(snapshot.interactions.len(), 1);
    let (_call, response) = &snapshot.interactions[0];

    let text = response.body["result"]["text"].as_str().unwrap();
    assert!(text.contains("Video"));
    assert!(text.contains("320 kbps"));
}
```

### Complex flow test
```rust
#[tokio::test]
async fn test_audio_download_flow() {
    let snapshot = TelegramSnapshot::load_by_name("audio_download_complete")?;

    // 5 steps: 0% -> 45% -> 100% -> sendAudio -> cleanup
    assert_eq!(snapshot.interactions.len(), 5);

    // Check progress
    let (_call1, resp1) = &snapshot.interactions[0];
    assert!(resp1.body["result"]["caption"].as_str().unwrap().contains("0%"));

    // Check file
    let (_call4, resp4) = &snapshot.interactions[3];
    let audio = &resp4.body["result"]["audio"];
    assert_eq!(audio["performer"].as_str().unwrap(), "Rick Astley");
}
```

## What Can Be Tested

### Commands
- `/start`, `/info`, `/settings`, `/help`
- Text, buttons, formatting verification

### Callback queries
- Language, quality, format selection
- answerCallbackQuery, message update verification

### Complex flows
- URL processing -> preview -> download -> send
- Multi-step interactions

### Error handling
- Rate limiting, invalid URLs, network errors
- Correct error message verification

### Operation progress
- Download progress updates
- editMessage operations

## Coverage Metrics

```
API methods covered:
  sendMessage        (6 snapshots)
  sendPhoto          (1 snapshot)
  sendAudio          (1 snapshot)
  deleteMessage      (2 snapshots)
  editMessageCaption (1 snapshot)
  editMessageText    (1 snapshot)
  answerCallbackQuery(1 snapshot)

Total: 7/20+ Bot API methods
```

## Extension

### Add new snapshots for:

1. **Video download** - `video_download_complete.json`
2. **Download history** - `downloads_list.json`
3. **Cuts** - `cuts_menu.json`, `cut_creation.json`
4. **Admin commands** - `admin_users_list.json`, `admin_backup.json`
5. **Subscriptions** - `subscription_purchase.json`
6. **Errors** - `invalid_url.json`, `network_error.json`

### Template for new snapshot:
```bash
cp tests/snapshots/start_command.json tests/snapshots/my_new_test.json
# Edit JSON
# Add test in tests/bot_commands_test.rs
```

## Next Steps

1. **Study** existing snapshots in [tests/snapshots/](tests/snapshots/)
2. **Run** tests: `cargo test --test bot_commands_test`
3. **Create** your snapshot for new functionality
4. **Add** test in `tests/bot_commands_test.rs`
5. **Verify**: `cargo test`

## Best Practices

- One snapshot = one scenario
- Descriptive file names
- Comments in metadata
- Minimal data (no extra fields)
- Version control in Git
- Regular updates when API changes

## Troubleshooting

**Snapshot doesn't load:**
```bash
# Check JSON
jq . tests/snapshots/my_test.json

# See error
cargo test test_my_snapshot -- --nocapture
```

**Test fails:**
```rust
// Add debugging
let snapshot = TelegramSnapshot::load_by_name("my_test")?;
println!("Loaded: {:?}", snapshot);
```

## Help

- Documentation: [docs/SNAPSHOT_TESTING.md](docs/SNAPSHOT_TESTING.md)
- Examples: [tests/bot_commands_test.rs](tests/bot_commands_test.rs)
- Index: [tests/snapshots/SNAPSHOT_INDEX.md](tests/snapshots/SNAPSHOT_INDEX.md)

---

**Status**: Fully working system
**Tests**: 18 passing
**Coverage**: Commands, settings, downloads, errors
