# Integrating Snapshot Testing with Real Bot Logic

## Question: Does real logic run in the tests?

**Short answer:** No, **by default it does NOT**. But you can easily add it!

## Current State

### What the existing tests do

```rust
#[tokio::test]
async fn test_start_command_from_snapshot() {
    let mock = TelegramMock::from_snapshot("start_command").await?;
    let bot = mock.create_bot()?;

    // Your code is NOT called!
    // Only the snapshot structure is verified
    assert_eq!(mock.snapshot().name, "start_command");
}
```

**What happens:**
- JSON with recorded API calls is loaded
- Mock Telegram server is created (wiremock)
- Data structure is verified
- **BUT:** your `handle_start_command` is NOT called
- **NOT verified:** that your code makes the correct calls

### This is useful for:

- **Snapshot validation** - check that JSON is correct
- **API documentation** - see what calls the bot makes
- **Structure regression tests** - ensure format has not changed

### But does NOT verify:

- That your `handle_start_command` works correctly
- That `/info` sends the correct message
- That URL processing makes the correct API calls

## How to Add Tests WITH Real Logic

### Option 1: Full integration (recommended)

```rust
use doradura::telegram::menu::show_main_menu;
use common::{TelegramMock, create_test_message};

#[tokio::test]
async fn test_start_command_calls_real_handler() {
    // 1. Load snapshot with EXPECTED calls
    let mock = TelegramMock::from_snapshot("start_command").await?;
    let bot = mock.create_bot()?;

    // 2. Prepare data
    let chat_id = ChatId(123456789);
    let db_pool = create_test_db_pool()?;

    // 3. CALL YOUR REAL FUNCTION!
    let result = show_main_menu(&bot, chat_id, &db_pool).await;

    // 4. Verify that function succeeded
    assert!(result.is_ok(), "show_main_menu should complete successfully");

    // 5. IMPORTANT: Verify that the CORRECT API calls were made
    mock.verify().await.expect("Function should have called sendMessage");
}
```

**What is verified:**
- Your function works without errors
- It makes the correct calls to the Telegram API
- Call structure matches the snapshot
- Parameters (text, chat_id, buttons) are correct

### Option 2: Command handler test

```rust
use doradura::telegram::commands::handle_info_command;

#[tokio::test]
async fn test_info_command_handler() {
    let mock = TelegramMock::from_snapshot("info_command").await?;
    let bot = mock.create_bot()?;

    // Create a fake "/info" message
    let message = create_test_message("/info", 123456789, 111222333);
    let db_pool = create_test_db_pool()?;

    // Call the handler
    let result = handle_info_command(&bot, message, &db_pool).await;
    assert!(result.is_ok());

    // Verify that the info message was sent
    mock.verify().await.expect("Info message should be sent");
}
```

### Option 3: Complex flow test

```rust
use doradura::telegram::commands::handle_message;

#[tokio::test]
async fn test_youtube_url_complete_flow() {
    // Snapshot contains 3 interactions
    let mock = TelegramMock::from_snapshot("youtube_processing").await?;
    let bot = mock.create_bot()?;

    let message = create_test_message(
        "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
        123456789,
        111222333
    );

    // Call URL handler
    let result = handle_message(
        bot.clone(),
        message,
        download_queue,
        rate_limiter,
        db_pool
    ).await;

    assert!(result.is_ok());

    // Verify call sequence:
    // 1. sendMessage("Processing...")
    // 2. sendPhoto(preview with buttons)
    // 3. deleteMessage(temporary message)
    mock.verify().await.expect("Flow should make 3 calls");
}
```

## Test Structure

### Level 1: Snapshot validation (exists now)

```
tests/bot_snapshots_test.rs
tests/bot_commands_test.rs
```

**Goal:** Verify that snapshots are valid and contain expected data

### Level 2: Integration tests (need to add)

```
tests/bot_integration_test.rs     ← NEW!
tests/commands_integration_test.rs ← NEW!
```

**Goal:** Call real handlers and verify API calls

### Level 3: End-to-end tests (optional)

```
tests/e2e/                         ← FUTURE
├── test_download_flow.rs
└── test_settings_flow.rs
```

**Goal:** Full cycle from command to result

## Example: Adding an Integration Test

### Step 1: Create snapshot (already exists)

```json
// tests/snapshots/info_command.json
{
  "name": "info_command",
  "interactions": [
    [
      {"method": "POST", "path": "/sendMessage", ...},
      {"status": 200, "body": {...}}
    ]
  ]
}
```

### Step 2: Write the test

```rust
// tests/commands_integration_test.rs

mod common;
use common::{TelegramMock, create_test_message};
use doradura::telegram::commands::handle_info_command;
use doradura::storage::create_pool;

#[tokio::test]
async fn test_info_command_sends_correct_message() {
    // Setup mock server
    let mock = TelegramMock::from_snapshot("info_command").await.unwrap();
    let bot = mock.create_bot().unwrap();

    // Setup test data
    let message = create_test_message("/info", 123456789, 111222333);
    let db_pool = create_pool(":memory:").unwrap();

    // Call REAL handler
    let result = handle_info_command(&bot, message, &db_pool).await;

    // Verify
    assert!(result.is_ok(), "Handler should succeed");
    mock.verify().await.expect("Should send info message");
}
```

### Step 3: Run

```bash
cargo test test_info_command_sends_correct_message
```

## What is Needed for Integration Tests

### 1. Test DB Setup

```rust
fn create_test_db_pool() -> anyhow::Result<Arc<DbPool>> {
    let pool = create_pool(":memory:")?;

    // Run migrations
    run_migrations(&pool)?;

    // Add test data
    insert_test_user(&pool, 123456789)?;

    Ok(Arc::new(pool))
}
```

### 2. Test Data Factories

```rust
fn create_test_user(id: i64) -> User { ... }
fn create_test_message(text: &str) -> Message { ... }
fn create_test_callback_query(data: &str) -> CallbackQuery { ... }
```

### 3. Assertions Helpers

```rust
fn assert_sent_message_with_text(mock: &TelegramMock, expected: &str) {
    let snapshot = mock.snapshot();
    let (call, _) = &snapshot.interactions[0];

    assert_eq!(call.path, "/sendMessage");
    assert!(call.body["text"].as_str().unwrap().contains(expected));
}
```

## Ready Template

A file [tests/bot_integration_test.rs](../tests/bot_integration_test.rs) with examples has been created!

```bash
# View the templates
cat tests/bot_integration_test.rs

# Uncomment code and run
cargo test --test bot_integration_test
```

## Project Setup for Integration Tests

### 1. Export needed functions

In `src/telegram/mod.rs`:

```rust
// Add pub use for tests
pub use commands::{handle_info_command, handle_message};
pub use menu::show_main_menu;
```

### 2. Add feature for tests (optional)

In `Cargo.toml`:

```toml
[features]
testing = []

[dev-dependencies]
# Already present
```

### 3. Create test utilities

```rust
// tests/common/test_db.rs
pub fn create_test_db() -> DbPool { ... }
pub fn insert_test_user(pool: &DbPool, id: i64) { ... }
```

## Approach Comparison

| Approach | What it verifies | Speed | Complexity |
|--------|---------------|----------|-----------|
| **Snapshot validation** | Data structure | Very fast | Simple |
| **Integration with mock** | Real logic + API calls | Fast | Medium |
| **E2E with real API** | Everything together | Slow | Complex |

## Recommendations

### Use both approaches:

1. **Snapshot validation** (exists) - fast structure check
2. **Integration tests** (add) - logic check

### Approximate ratio:

- 70% of tests - snapshot validation (fast)
- 30% of tests - integration with real logic (important flows)

### Integration priorities:

1. Critical commands (`/start`, `/info`)
2. Complex flows (download, settings)
3. Error handling (rate limit, invalid URL)
4. Edge cases (as needed)

## Next Steps

1. **Study** [tests/bot_integration_test.rs](../tests/bot_integration_test.rs)
2. **Uncomment** one of the examples
3. **Add** missing dependencies (DB setup)
4. **Run** the test
5. **Expand** coverage

## See Also

- [SNAPSHOT_TESTING.md](SNAPSHOT_TESTING.md) - general documentation
- [tests/bot_integration_test.rs](../tests/bot_integration_test.rs) - code examples
- [tests/common/helpers.rs](../tests/common/helpers.rs) - test utilities
