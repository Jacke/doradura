# Telegram Bot Snapshots

This directory contains recorded Telegram API interactions for testing.

## What is a snapshot?

A snapshot is a JSON file containing:
- Requests to the Telegram API (method, path, body)
- Responses from the Telegram API (status, body, headers)
- Metadata (scenario description, recording date)

## Snapshot file structure

```json
{
  "name": "scenario_name",
  "version": "1.0",
  "recorded_at": "2026-01-04T12:00:00Z",
  "interactions": [
    [
      {
        "method": "POST",
        "path": "/sendMessage",
        "body": { "chat_id": 123, "text": "Hello" },
        "timestamp": 1735992000
      },
      {
        "status": 200,
        "body": { "ok": true, "result": {...} },
        "headers": { "content-type": "application/json" }
      }
    ]
  ],
  "metadata": {
    "description": "Scenario description",
    "command": "/start"
  }
}
```

## Existing snapshots

### start_command.json
- **Description**: User sends /start and receives welcome message with main menu
- **Command**: `/start`
- **Interactions**: 1
- **Usage**:
  ```rust
  let mock = TelegramMock::from_snapshot("start_command").await?;
  ```

### info_command.json
- **Description**: User requests information about supported formats
- **Command**: `/info`
- **Interactions**: 1
- **Includes**: List of video/audio formats, supported services

### settings_menu.json
- **Description**: Display of main settings menu with current preferences
- **Command**: `/settings`
- **Interactions**: 1
- **Includes**: Video quality, audio bitrate, default format, language

### language_selection.json
- **Description**: Complete language selection flow
- **Flow**: Show language menu → select → callback → update settings
- **Interactions**: 3
- **Includes**: answerCallbackQuery, editMessageText

### youtube_processing.json
- **Description**: Processing YouTube link and showing preview with quality options
- **Flow**: "Processing..." message → Preview with thumbnail → Delete temporary message
- **Interactions**: 3
- **Includes**: sendMessage, sendPhoto, deleteMessage
- **URL**: `https://www.youtube.com/watch?v=dQw4w9WgXcQ`

### audio_download_complete.json
- **Description**: Complete audio download cycle with progress tracking
- **Flow**: 0% → 45% → 100% → send file → cleanup
- **Interactions**: 5
- **Includes**: editMessageCaption (progress), sendAudio, deleteMessage
- **Details**: Rick Astley - Never Gonna Give You Up, 192kbps, 5MB

### rate_limit_error.json
- **Description**: User exceeds the rate limit
- **Interactions**: 1
- **Includes**: Error message with remaining time (45 sec)
- **Error type**: rate_limit

## How to create a new snapshot

### Method 1: Manually (recommended)

1. Run the bot with logging:
   ```bash
   RUST_LOG=debug cargo run
   ```

2. Perform the required action in Telegram

3. Copy the request/response from the logs

4. Create a JSON file in `tests/snapshots/`

5. Use it in tests

### Method 2: Python utility

```bash
# From logs
./tools/log_to_snapshot.py --input bot.log --name my_test --output tests/snapshots/my_test.json

# From stdin
cargo run 2>&1 | ./tools/log_to_snapshot.py --stdin --name my_test

# Interactively
./tools/log_to_snapshot.py --interactive
```

### Method 3: Via mitmproxy

```bash
# Configure proxy
mitmproxy --port 8080 --mode reverse:http://localhost:8081

# In .env
BOT_API_URL=http://localhost:8080

# Use the bot
cargo run

# Save flows from mitmproxy
```

## Naming conventions

- `{command}_command.json` - for bot commands (`start_command.json`)
- `{action}_callback.json` - for callback buttons (`settings_callback.json`)
- `{feature}_flow.json` - for complex scenarios (`youtube_download_flow.json`)
- `{error}_error.json` - for errors (`invalid_url_error.json`)

## Testing with snapshots

```rust
use common::TelegramMock;

#[tokio::test]
async fn test_my_feature() {
    let mock = TelegramMock::from_snapshot("my_snapshot").await?;
    let bot = mock.create_bot()?;

    // Your test code
    // The bot will use the mock server instead of the real API

    mock.verify().await?; // Verify that all expected calls were made
}
```

## Detailed documentation

See [docs/SNAPSHOT_TESTING.md](../../docs/SNAPSHOT_TESTING.md)
