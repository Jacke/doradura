# Quick Start: Snapshot Testing

## What is this?

A system for recording and replaying real Telegram API interactions in tests.

## In 5 Minutes

### 1. Record an interaction

```bash
# Enable logging
RUST_LOG=debug cargo run

# Send a command to the bot (e.g. /start)
# Copy the JSON from the logs
```

### 2. Create a snapshot

```bash
./tools/log_to_snapshot.py --interactive
```

Or manually create `tests/snapshots/my_test.json`:

```json
{
  "name": "my_test",
  "version": "1.0",
  "recorded_at": "2026-01-04T12:00:00Z",
  "interactions": [
    [
      {
        "method": "POST",
        "path": "/sendMessage",
        "body": {"chat_id": 123, "text": "Hello"},
        "timestamp": 1735992000
      },
      {
        "status": 200,
        "body": {"ok": true, "result": {...}},
        "headers": {"content-type": "application/json"}
      }
    ]
  ],
  "metadata": {}
}
```

### 3. Use in a test

Add to `tests/bot_test.rs`:

```rust
mod common;
use common::TelegramMock;

#[tokio::test]
async fn test_my_feature() {
    let mock = TelegramMock::from_snapshot("my_test").await.unwrap();
    let bot = mock.create_bot().unwrap();

    // Your testing code here
    // bot.send_message(...).await?;

    // mock.verify().await.unwrap(); // Optional
}
```

### 4. Run the test

```bash
cargo test --test bot_test
```

## Examples

### Test for /start command

```rust
#[tokio::test]
async fn test_start_command() {
    let mock = TelegramMock::from_snapshot("start_command").await.unwrap();
    let bot = mock.create_bot().unwrap();

    // Call your handler
    // handle_start_command(&bot, message).await?;

    // Assertions
    assert_eq!(mock.snapshot().interactions.len(), 1);
}
```

### Test for video download

```rust
#[tokio::test]
async fn test_youtube_download() {
    let mock = TelegramMock::from_snapshot("youtube_download").await.unwrap();
    let bot = mock.create_bot().unwrap();

    // Full flow: preview -> quality selection -> download
    // ...
}
```

## Project Structure

```
doradura/
├── src/
│   └── testing/          # (for unit tests only)
├── tests/
│   ├── common/           # Shared testing utilities
│   │   ├── snapshots.rs  # Snapshot loading/replay
│   │   └── recorder.rs   # Recording utilities
│   ├── snapshots/        # JSON snapshots
│   │   ├── start_command.json
│   │   └── README.md
│   └── bot_snapshots_test.rs  # Tests
├── tools/
│   └── log_to_snapshot.py     # Converter
└── docs/
    └── SNAPSHOT_TESTING.md    # Full docs
```

## Advantages

- Fast tests (no real API calls)
- Deterministic (always the same result)
- Work offline
- Document API interactions
- Easy to create new tests

## Further Reading

- [Full documentation](SNAPSHOT_TESTING.md)
- [Test examples](../tests/bot_snapshots_test.rs)
- [Existing snapshots](../tests/snapshots/)
