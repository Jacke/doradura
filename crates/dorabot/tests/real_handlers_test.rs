//! Integration tests for REAL Telegram handlers using TelegramMock (wiremock)
//!
//! These tests execute the actual handler code from src/telegram/handlers.rs
//! with mocked Telegram API responses.
//!
//! Run with: cargo test --test real_handlers_test

use serial_test::serial;
use std::sync::Arc;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

use doradura::core::rate_limiter::RateLimiter;
use doradura::download::DownloadQueue;
use doradura::downsub::DownsubGateway;
use doradura::storage::create_pool;
use doradura::telegram::bot_api_logger::Bot as CustomBot;
use doradura::telegram::{schema, HandlerDeps, HandlerError};
use teloxide::prelude::*;
use teloxide::types::{CallbackQuery, Message};

/// Test harness for real handler testing
struct RealHandlerTest {
    mock_server: MockServer,
    bot: CustomBot,
    deps: HandlerDeps,
    db_path: String,
}

impl Drop for RealHandlerTest {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.db_path);
        let _ = std::fs::remove_file(format!("{}-journal", &self.db_path));
        let _ = std::fs::remove_file(format!("{}-wal", &self.db_path));
        let _ = std::fs::remove_file(format!("{}-shm", &self.db_path));
    }
}

impl RealHandlerTest {
    /// Create a new test harness with mock server and real dependencies
    async fn new() -> Self {
        let mock_server = MockServer::start().await;

        // Create teloxide Bot pointing to mock server
        let teloxide_bot =
            teloxide::Bot::new("test_token_12345:ABCDEF").set_api_url(mock_server.uri().parse().unwrap());

        // Wrap with our custom Bot (for logging)
        let bot = CustomBot::new(teloxide_bot);

        // Create file-based temp database (shared across all pool connections)
        // Using process ID + timestamp + random suffix for uniqueness across parallel tests
        let db_path = format!(
            "/tmp/doradura_test_{}_{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let db_pool = Arc::new(create_pool(&db_path).expect("Failed to create test database"));

        // Initialize database schema with all required tables
        {
            let conn = db_pool.get().expect("Failed to get connection");
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS users (
                    id INTEGER PRIMARY KEY,
                    telegram_id INTEGER NOT NULL UNIQUE,
                    username TEXT,
                    language TEXT DEFAULT 'ru',
                    created_at TEXT DEFAULT CURRENT_TIMESTAMP,
                    updated_at TEXT DEFAULT CURRENT_TIMESTAMP
                );

                CREATE TABLE IF NOT EXISTS user_settings (
                    id INTEGER PRIMARY KEY,
                    user_id INTEGER NOT NULL UNIQUE,
                    download_type TEXT DEFAULT 'mp3',
                    video_quality TEXT DEFAULT '720p',
                    audio_bitrate TEXT DEFAULT '320k',
                    FOREIGN KEY (user_id) REFERENCES users(telegram_id)
                );

                CREATE TABLE IF NOT EXISTS downloads (
                    id INTEGER PRIMARY KEY,
                    user_id INTEGER,
                    url TEXT,
                    title TEXT,
                    format TEXT,
                    file_size INTEGER,
                    duration INTEGER,
                    telegram_file_id TEXT,
                    message_id INTEGER,
                    created_at TEXT DEFAULT CURRENT_TIMESTAMP
                );

                CREATE TABLE IF NOT EXISTS subscriptions (
                    id INTEGER PRIMARY KEY,
                    user_id INTEGER NOT NULL UNIQUE,
                    plan TEXT DEFAULT 'free',
                    started_at TEXT,
                    expires_at TEXT,
                    FOREIGN KEY (user_id) REFERENCES users(telegram_id)
                );

                CREATE TABLE IF NOT EXISTS request_history (
                    id INTEGER PRIMARY KEY,
                    user_id INTEGER NOT NULL,
                    request_text TEXT,
                    created_at TEXT DEFAULT CURRENT_TIMESTAMP
                );

                CREATE TABLE IF NOT EXISTS uploads (
                    id INTEGER PRIMARY KEY,
                    user_id INTEGER NOT NULL,
                    original_filename TEXT,
                    title TEXT NOT NULL,
                    media_type TEXT NOT NULL,
                    file_format TEXT,
                    file_id TEXT NOT NULL,
                    file_unique_id TEXT,
                    file_size INTEGER,
                    duration INTEGER,
                    width INTEGER,
                    height INTEGER,
                    mime_type TEXT,
                    message_id INTEGER,
                    chat_id INTEGER,
                    thumbnail_file_id TEXT,
                    uploaded_at TEXT DEFAULT CURRENT_TIMESTAMP
                );
                "#,
            )
            .expect("Failed to create tables");
        }

        let download_queue = Arc::new(DownloadQueue::new());
        let rate_limiter = Arc::new(RateLimiter::new());
        let downsub_gateway = Arc::new(DownsubGateway::from_env());

        let extension_registry = Arc::new(doradura::extension::ExtensionRegistry::default_registry());

        let subtitle_cache = Arc::new(doradura::storage::SubtitleCache::new("/tmp/doradura_test_subtitles"));

        let deps = HandlerDeps::new(
            db_pool,
            download_queue,
            rate_limiter,
            downsub_gateway,
            subtitle_cache,
            Some("test_bot".to_string()),
            UserId(987654321),
            None, // alert_manager - not needed for tests
            extension_registry,
        );

        Self {
            mock_server,
            bot,
            deps,
            db_path,
        }
    }

    /// Create a test user in the database
    fn create_test_user(&self, telegram_id: i64, username: &str, language: &str) {
        let conn = self.deps.db_pool.get().expect("Failed to get connection");
        conn.execute(
            "INSERT INTO users (telegram_id, username, language) VALUES (?1, ?2, ?3)",
            rusqlite::params![telegram_id, username, language],
        )
        .expect("Failed to insert test user");

        // Also create user settings
        conn.execute(
            "INSERT INTO user_settings (user_id) VALUES (?1)",
            rusqlite::params![telegram_id],
        )
        .expect("Failed to insert user settings");
    }

    /// Create a test upload entry in the database
    fn create_test_upload(&self, user_id: i64, title: &str, media_type: &str, file_id: &str) -> i64 {
        let conn = self.deps.db_pool.get().expect("Failed to get connection");
        conn.execute(
            "INSERT INTO uploads (user_id, title, media_type, file_id, file_unique_id, file_size, duration)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![user_id, title, media_type, file_id, "unique_123", 1024 * 1024, 120],
        )
        .expect("Failed to insert test upload");
        conn.last_insert_rowid()
    }

    /// Mock a sendMessage API call
    #[allow(dead_code)]
    async fn mock_send_message(&self, response_text: &str) {
        let response = serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 42,
                "from": {
                    "id": 987654321,
                    "is_bot": true,
                    "first_name": "TestBot",
                    "username": "test_bot"
                },
                "chat": {
                    "id": 123456789,
                    "first_name": "Test",
                    "username": "testuser",
                    "type": "private"
                },
                "date": 1735992000,
                "text": response_text
            }
        });

        Mock::given(method("POST"))
            .and(path_regex("(?i)/bot[^/]+/sendMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .mount(&self.mock_server)
            .await;
    }

    /// Mock an editMessageText API call
    #[allow(dead_code)]
    async fn mock_edit_message(&self) {
        let response = serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 42,
                "from": {
                    "id": 987654321,
                    "is_bot": true,
                    "first_name": "TestBot"
                },
                "chat": {
                    "id": 123456789,
                    "type": "private"
                },
                "date": 1735992000,
                "text": "Edited message"
            }
        });

        Mock::given(method("POST"))
            .and(path_regex("(?i)/bot[^/]+/editMessageText"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .mount(&self.mock_server)
            .await;
    }

    /// Mock an answerCallbackQuery API call
    #[allow(dead_code)]
    async fn mock_answer_callback(&self) {
        let response = serde_json::json!({
            "ok": true,
            "result": true
        });

        Mock::given(method("POST"))
            .and(path_regex("(?i)/bot[^/]+/answerCallbackQuery"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .mount(&self.mock_server)
            .await;
    }

    /// Mock setMyCommands API call
    #[allow(dead_code)]
    async fn mock_set_commands(&self) {
        let response = serde_json::json!({
            "ok": true,
            "result": true
        });

        Mock::given(method("POST"))
            .and(path_regex("(?i)/bot[^/]+/setMyCommands"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .mount(&self.mock_server)
            .await;
    }

    /// Mock sendVoice API call
    #[allow(dead_code)]
    async fn mock_send_voice(&self) {
        let response = serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 43,
                "from": {
                    "id": 987654321,
                    "is_bot": true,
                    "first_name": "TestBot"
                },
                "chat": {
                    "id": 123456789,
                    "type": "private"
                },
                "date": 1735992000,
                "voice": {
                    "file_id": "test_voice_id",
                    "file_unique_id": "unique_id",
                    "duration": 5
                }
            }
        });

        Mock::given(method("POST"))
            .and(path_regex("(?i)/bot[^/]+/sendVoice"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .mount(&self.mock_server)
            .await;
    }

    /// Mock ALL common Telegram API calls (catch-all for tests)
    async fn mock_all_telegram_api(&self) {
        // sendMessage
        let send_msg = serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 42,
                "from": { "id": 987654321, "is_bot": true, "first_name": "TestBot" },
                "chat": { "id": 123456789, "type": "private" },
                "date": 1735992000,
                "text": "Response"
            }
        });
        Mock::given(method("POST"))
            .and(path_regex("(?i)/bot[^/]+/sendMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(send_msg.clone()))
            .mount(&self.mock_server)
            .await;

        // editMessageText
        Mock::given(method("POST"))
            .and(path_regex("(?i)/bot[^/]+/editMessageText"))
            .respond_with(ResponseTemplate::new(200).set_body_json(send_msg.clone()))
            .mount(&self.mock_server)
            .await;

        // editMessageCaption
        Mock::given(method("POST"))
            .and(path_regex("(?i)/bot[^/]+/editMessageCaption"))
            .respond_with(ResponseTemplate::new(200).set_body_json(send_msg.clone()))
            .mount(&self.mock_server)
            .await;

        // editMessageReplyMarkup
        Mock::given(method("POST"))
            .and(path_regex("(?i)/bot[^/]+/editMessageReplyMarkup"))
            .respond_with(ResponseTemplate::new(200).set_body_json(send_msg.clone()))
            .mount(&self.mock_server)
            .await;

        // answerCallbackQuery
        let answer_cb = serde_json::json!({ "ok": true, "result": true });
        Mock::given(method("POST"))
            .and(path_regex("(?i)/bot[^/]+/answerCallbackQuery"))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer_cb))
            .mount(&self.mock_server)
            .await;

        // setMyCommands
        let set_cmds = serde_json::json!({ "ok": true, "result": true });
        Mock::given(method("POST"))
            .and(path_regex("(?i)/bot[^/]+/setMyCommands"))
            .respond_with(ResponseTemplate::new(200).set_body_json(set_cmds))
            .mount(&self.mock_server)
            .await;

        // deleteMessage
        let delete_msg = serde_json::json!({ "ok": true, "result": true });
        Mock::given(method("POST"))
            .and(path_regex("(?i)/bot[^/]+/deleteMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(delete_msg))
            .mount(&self.mock_server)
            .await;

        // sendVoice
        let voice = serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 43,
                "from": { "id": 987654321, "is_bot": true, "first_name": "TestBot" },
                "chat": { "id": 123456789, "type": "private" },
                "date": 1735992000,
                "voice": { "file_id": "voice_id", "file_unique_id": "uid", "duration": 5 }
            }
        });
        Mock::given(method("POST"))
            .and(path_regex("(?i)/bot[^/]+/sendVoice"))
            .respond_with(ResponseTemplate::new(200).set_body_json(voice))
            .mount(&self.mock_server)
            .await;

        // sendPhoto
        let photo = serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 44,
                "from": { "id": 987654321, "is_bot": true, "first_name": "TestBot" },
                "chat": { "id": 123456789, "type": "private" },
                "date": 1735992000,
                "photo": [{ "file_id": "photo_id", "file_unique_id": "uid", "width": 100, "height": 100 }]
            }
        });
        Mock::given(method("POST"))
            .and(path_regex("(?i)/bot[^/]+/sendPhoto"))
            .respond_with(ResponseTemplate::new(200).set_body_json(photo))
            .mount(&self.mock_server)
            .await;

        // getMe
        let get_me = serde_json::json!({
            "ok": true,
            "result": {
                "id": 987654321,
                "is_bot": true,
                "first_name": "TestBot",
                "username": "test_bot",
                "can_join_groups": true,
                "can_read_all_group_messages": false,
                "supports_inline_queries": false
            }
        });
        Mock::given(method("POST"))
            .and(path_regex("(?i)/bot[^/]+/getMe"))
            .respond_with(ResponseTemplate::new(200).set_body_json(get_me))
            .mount(&self.mock_server)
            .await;

        // Catch-all for any unhandled POST requests - returns a valid "ok" response
        // Use lowest priority (255) so specific mocks above always win
        let fallback = serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 999,
                "from": { "id": 987654321, "is_bot": true, "first_name": "TestBot" },
                "chat": { "id": 123456789, "type": "private" },
                "date": 1735992000,
                "text": "Fallback response"
            }
        });
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fallback.clone()))
            .with_priority(255)
            .mount(&self.mock_server)
            .await;

        // Also catch GET requests
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fallback))
            .with_priority(255)
            .mount(&self.mock_server)
            .await;
    }

    /// Create a Message from JSON (more reliable than struct construction)
    fn create_message_from_json(text: &str, chat_id: i64, user_id: u64) -> Message {
        let json = serde_json::json!({
            "message_id": 1,
            "date": 1735992000,
            "chat": {
                "id": chat_id,
                "type": "private",
                "first_name": "Test",
                "username": "testuser"
            },
            "from": {
                "id": user_id,
                "is_bot": false,
                "first_name": "Test",
                "username": "testuser",
                "language_code": "ru"
            },
            "text": text
        });

        serde_json::from_value(json).expect("Failed to deserialize message")
    }

    /// Create a CallbackQuery from JSON
    fn create_callback_from_json(data: &str, chat_id: i64, user_id: u64) -> CallbackQuery {
        let json = serde_json::json!({
            "id": "callback_123",
            "from": {
                "id": user_id,
                "is_bot": false,
                "first_name": "Test",
                "username": "testuser",
                "language_code": "ru"
            },
            "message": {
                "message_id": 42,
                "date": 1735992000,
                "chat": {
                    "id": chat_id,
                    "type": "private",
                    "first_name": "Test",
                    "username": "testuser"
                },
                "from": {
                    "id": 987654321,
                    "is_bot": true,
                    "first_name": "TestBot",
                    "username": "test_bot"
                },
                "text": "Original message"
            },
            "chat_instance": "chat_instance_123",
            "data": data
        });

        serde_json::from_value(json).expect("Failed to deserialize callback")
    }

    /// Get the handler schema with deps
    fn handler(&self) -> teloxide::dispatching::UpdateHandler<HandlerError> {
        schema(self.deps.clone())
    }

    /// Get the bot
    fn bot(&self) -> &CustomBot {
        &self.bot
    }
}

// =============================================================================
// TESTS - Direct function calls with mocked API
// =============================================================================

/// Test show_language_selection_menu - REAL FUNCTION
#[tokio::test]
#[serial]
async fn test_real_show_language_selection_menu() {
    use doradura::telegram::show_language_selection_menu;

    let test = RealHandlerTest::new().await;

    // Mock all Telegram API calls
    test.mock_all_telegram_api().await;

    // Call the REAL function
    let result = show_language_selection_menu(test.bot(), ChatId(123456789)).await;
    assert!(result.is_ok(), "show_language_selection_menu should succeed");

    // Verify the request
    let requests = test.mock_server.received_requests().await.unwrap();
    let send_msg = requests
        .iter()
        .find(|r| r.url.path().to_lowercase().contains("sendmessage"))
        .expect("Should have sendMessage request");

    let body: serde_json::Value = serde_json::from_slice(&send_msg.body).unwrap();

    // Verify keyboard has language buttons
    let keyboard = &body["reply_markup"]["inline_keyboard"];
    assert!(keyboard.is_array(), "Should have inline_keyboard");

    let rows = keyboard.as_array().unwrap();
    assert!(rows.len() >= 2, "Should have at least 2 language options");

    // Verify first button has language callback
    let first_button = &rows[0][0];
    let callback_data = first_button["callback_data"].as_str().unwrap();
    assert!(
        callback_data.starts_with("language:"),
        "Should have language callback, got: {}",
        callback_data
    );

    // Verify button text contains flag emoji
    let button_text = first_button["text"].as_str().unwrap();
    assert!(
        button_text.contains("🇷🇺") || button_text.contains("🇺🇸"),
        "Should have flag emoji in button"
    );

    println!(
        "✅ show_language_selection_menu: verified {} language options",
        rows.len()
    );
}

/// Test show_main_menu - REAL FUNCTION
#[tokio::test]
#[serial]
async fn test_real_show_main_menu() {
    use doradura::telegram::show_main_menu;

    let test = RealHandlerTest::new().await;

    // Create user in DB (required for main menu)
    test.create_test_user(123456789, "testuser", "ru");

    // Mock all Telegram API calls
    test.mock_all_telegram_api().await;

    // Call the REAL function
    let result = show_main_menu(test.bot(), ChatId(123456789), test.deps.db_pool.clone()).await;
    assert!(result.is_ok(), "show_main_menu should succeed");

    // Verify what was sent to Telegram API
    let requests = test.mock_server.received_requests().await.unwrap();
    assert!(!requests.is_empty(), "Should have sent at least one request");

    // Find sendMessage request
    let send_msg_request = requests
        .iter()
        .find(|r| r.url.path().to_lowercase().contains("sendmessage"))
        .expect("Should have sendMessage request");

    let body: serde_json::Value = serde_json::from_slice(&send_msg_request.body).expect("Body should be valid JSON");

    // Verify reply_markup exists (inline keyboard)
    assert!(
        body.get("reply_markup").is_some(),
        "Should have reply_markup (inline keyboard)"
    );

    let reply_markup = &body["reply_markup"];
    let keyboard = &reply_markup["inline_keyboard"];
    assert!(keyboard.is_array(), "Should have inline_keyboard array");

    let rows = keyboard.as_array().unwrap();
    assert!(rows.len() >= 2, "Should have at least 2 rows of buttons");

    // Verify first row has quality button
    let first_row = rows[0].as_array().unwrap();
    assert!(!first_row.is_empty(), "First row should have buttons");

    // Check callback_data format
    let first_button = &first_row[0];
    assert!(
        first_button.get("callback_data").is_some(),
        "Button should have callback_data"
    );

    println!("✅ show_main_menu: verified keyboard with {} rows", rows.len());
}

/// Test show_enhanced_main_menu - REAL FUNCTION
#[tokio::test]
#[serial]
async fn test_real_show_enhanced_main_menu() {
    use doradura::telegram::show_enhanced_main_menu;

    let test = RealHandlerTest::new().await;

    // Create user
    test.create_test_user(123456789, "testuser", "ru");

    // Mock all Telegram API calls
    test.mock_all_telegram_api().await;

    // Call the REAL function
    let result = show_enhanced_main_menu(test.bot(), ChatId(123456789), test.deps.db_pool.clone()).await;
    assert!(result.is_ok(), "show_enhanced_main_menu should succeed");

    // Verify the request
    let requests = test.mock_server.received_requests().await.unwrap();
    let send_msg = requests
        .iter()
        .find(|r| r.url.path().to_lowercase().contains("sendmessage"))
        .expect("Should have sendMessage request");

    let body: serde_json::Value = serde_json::from_slice(&send_msg.body).unwrap();

    // Verify keyboard structure
    let keyboard = &body["reply_markup"]["inline_keyboard"];
    assert!(keyboard.is_array(), "Should have inline_keyboard");

    let rows = keyboard.as_array().unwrap();
    assert!(rows.len() >= 3, "Enhanced menu should have at least 3 rows");

    // Verify text contains expected content (MarkdownV2 format)
    let text = body["text"].as_str().unwrap();
    assert!(!text.is_empty(), "Should have message text");

    println!(
        "✅ show_enhanced_main_menu: verified {} rows, text len={}",
        rows.len(),
        text.len()
    );
}

/// Test handle_info_command - REAL FUNCTION
#[tokio::test]
#[serial]
async fn test_real_handle_info_command() {
    use doradura::telegram::commands::handle_info_command;

    let test = RealHandlerTest::new().await;

    // Create user
    test.create_test_user(123456789, "testuser", "ru");

    // Mock all Telegram API calls
    test.mock_all_telegram_api().await;

    // Create message from JSON
    let message = RealHandlerTest::create_message_from_json("/info", 123456789, 123456789);

    // Call the REAL function
    let result = handle_info_command(test.bot().clone(), message, test.deps.db_pool.clone()).await;
    assert!(result.is_ok(), "handle_info_command should succeed");

    // Verify the request
    let requests = test.mock_server.received_requests().await.unwrap();
    assert!(!requests.is_empty(), "Should have sent requests");

    // Find sendMessage request
    let send_msg = requests
        .iter()
        .find(|r| r.url.path().to_lowercase().contains("sendmessage"))
        .expect("Should have sendMessage request");

    let body: serde_json::Value = serde_json::from_slice(&send_msg.body).unwrap();

    // Verify message has text
    let text = body["text"].as_str().or_else(|| body["caption"].as_str()).unwrap_or("");
    assert!(!text.is_empty(), "Should have message text");

    println!("✅ handle_info_command: sent message with {} chars", text.len());
}

/// Test handle_menu_callback - REAL FUNCTION
#[tokio::test]
#[serial]
async fn test_real_handle_menu_callback() {
    use doradura::telegram::handle_menu_callback;

    let test = RealHandlerTest::new().await;

    // Create user
    test.create_test_user(123456789, "testuser", "ru");

    // Mock all Telegram API calls
    test.mock_all_telegram_api().await;

    // Create callback from JSON - use valid callback data
    let callback = RealHandlerTest::create_callback_from_json("main:settings", 123456789, 123456789);

    // Call the REAL function
    let result = handle_menu_callback(
        test.bot().clone(),
        callback,
        test.deps.db_pool.clone(),
        test.deps.download_queue.clone(),
        test.deps.rate_limiter.clone(),
        test.deps.extension_registry.clone(),
        test.deps.downsub_gateway.clone(),
        test.deps.subtitle_cache.clone(),
    )
    .await;
    assert!(result.is_ok(), "handle_menu_callback should succeed");

    // Verify API calls
    let requests = test.mock_server.received_requests().await.unwrap();

    // Should answer the callback query
    let has_answer = requests
        .iter()
        .any(|r| r.url.path().to_lowercase().contains("answercallbackquery"));
    assert!(has_answer, "Should answer callback query");

    // Should edit or send message (menu:back shows main menu)
    let has_message = requests.iter().any(|r| {
        let path = r.url.path().to_lowercase();
        path.contains("sendmessage") || path.contains("editmessage")
    });
    assert!(has_message, "Should send or edit message");

    println!("✅ handle_menu_callback (menu:back): {} API calls", requests.len());
}

/// Test settings menu callback - REAL FUNCTION
#[tokio::test]
#[serial]
async fn test_real_settings_callback() {
    use doradura::telegram::handle_menu_callback;

    let test = RealHandlerTest::new().await;

    // Create user
    test.create_test_user(123456789, "testuser", "ru");

    // Mock all Telegram API calls
    test.mock_all_telegram_api().await;

    // Create callback for settings - use mode: prefix
    let callback = RealHandlerTest::create_callback_from_json("mode:video_quality", 123456789, 123456789);

    // Call the REAL function
    let result = handle_menu_callback(
        test.bot().clone(),
        callback,
        test.deps.db_pool.clone(),
        test.deps.download_queue.clone(),
        test.deps.rate_limiter.clone(),
        test.deps.extension_registry.clone(),
        test.deps.downsub_gateway.clone(),
        test.deps.subtitle_cache.clone(),
    )
    .await;
    assert!(result.is_ok(), "settings callback should succeed");

    // Verify API calls
    let requests = test.mock_server.received_requests().await.unwrap();

    // Should answer callback
    let has_answer = requests
        .iter()
        .any(|r| r.url.path().to_lowercase().contains("answercallbackquery"));
    assert!(has_answer, "Should answer callback query");

    // Find the edit/send message with settings keyboard
    let msg_request = requests.iter().find(|r| {
        let path = r.url.path().to_lowercase();
        path.contains("editmessage") || path.contains("sendmessage")
    });
    assert!(msg_request.is_some(), "Should edit or send settings message");

    // Verify settings menu has keyboard
    if let Some(req) = msg_request {
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        let has_markup = body.get("reply_markup").is_some();
        assert!(has_markup, "Settings should have inline keyboard");
    }

    println!("✅ settings callback: {} API calls with keyboard", requests.len());
}

/// Test downloads menu callback - REAL FUNCTION
#[tokio::test]
#[serial]
async fn test_real_downloads_callback() {
    use doradura::telegram::handle_menu_callback;

    let test = RealHandlerTest::new().await;

    // Create user
    test.create_test_user(123456789, "testuser", "ru");

    // Mock all Telegram API calls
    test.mock_all_telegram_api().await;

    // Create callback for history (downloads history)
    let callback = RealHandlerTest::create_callback_from_json("main:history", 123456789, 123456789);

    // Call the REAL function
    let result = handle_menu_callback(
        test.bot().clone(),
        callback,
        test.deps.db_pool.clone(),
        test.deps.download_queue.clone(),
        test.deps.rate_limiter.clone(),
        test.deps.extension_registry.clone(),
        test.deps.downsub_gateway.clone(),
        test.deps.subtitle_cache.clone(),
    )
    .await;
    assert!(result.is_ok(), "downloads callback should succeed");

    // Verify API calls
    let requests = test.mock_server.received_requests().await.unwrap();

    // Should answer callback
    let has_answer = requests
        .iter()
        .any(|r| r.url.path().to_lowercase().contains("answercallbackquery"));
    assert!(has_answer, "Should answer callback query");

    // Find message request
    let msg_request = requests.iter().find(|r| {
        let path = r.url.path().to_lowercase();
        path.contains("editmessage") || path.contains("sendmessage")
    });
    assert!(msg_request.is_some(), "Should send downloads message");

    // Verify message was sent
    if let Some(req) = msg_request {
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        let has_text = body.get("text").is_some() || body.get("caption").is_some();
        assert!(has_text, "History should have text content");
    }

    println!("✅ history callback: {} API calls", requests.len());
}

/// Test info menu callback - REAL FUNCTION
#[tokio::test]
#[serial]
async fn test_real_info_callback() {
    use doradura::telegram::handle_menu_callback;

    let test = RealHandlerTest::new().await;

    // Create user
    test.create_test_user(123456789, "testuser", "ru");

    // Mock all Telegram API calls
    test.mock_all_telegram_api().await;

    // Create callback for stats (info/statistics)
    let callback = RealHandlerTest::create_callback_from_json("main:stats", 123456789, 123456789);

    // Call the REAL function
    let result = handle_menu_callback(
        test.bot().clone(),
        callback,
        test.deps.db_pool.clone(),
        test.deps.download_queue.clone(),
        test.deps.rate_limiter.clone(),
        test.deps.extension_registry.clone(),
        test.deps.downsub_gateway.clone(),
        test.deps.subtitle_cache.clone(),
    )
    .await;
    assert!(result.is_ok(), "info callback should succeed");

    // Verify API calls
    let requests = test.mock_server.received_requests().await.unwrap();

    // Should answer callback
    let has_answer = requests
        .iter()
        .any(|r| r.url.path().to_lowercase().contains("answercallbackquery"));
    assert!(has_answer, "Should answer callback query");

    // Find message request
    let msg_request = requests.iter().find(|r| {
        let path = r.url.path().to_lowercase();
        path.contains("editmessage") || path.contains("sendmessage")
    });
    assert!(msg_request.is_some(), "Should send info message");

    // Verify message was sent/edited
    if let Some(req) = msg_request {
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();

        // Check for text or caption (in case of edit)
        let has_text = body.get("text").is_some() || body.get("caption").is_some();
        assert!(has_text, "Stats should have text content");
    }

    println!("✅ stats callback: {} API calls", requests.len());
}

/// Test handler schema can be built
#[tokio::test]
#[serial]
async fn test_handler_schema_builds() {
    let test = RealHandlerTest::new().await;

    // Just verify the handler can be built with real deps
    let _handler = test.handler();

    println!("✅ Handler schema built successfully with real dependencies");
}

/// Test database setup is correct
#[tokio::test]
#[serial]
async fn test_database_setup() {
    let test = RealHandlerTest::new().await;

    // Create user
    test.create_test_user(123456789, "testuser", "ru");

    // Verify user exists
    let conn = test.deps.db_pool.get().unwrap();
    let count: i32 = conn
        .query_row("SELECT COUNT(*) FROM users WHERE telegram_id = 123456789", [], |row| {
            row.get(0)
        })
        .unwrap();

    assert_eq!(count, 1, "User should exist in database");

    // Verify user settings exist
    let settings_count: i32 = conn
        .query_row(
            "SELECT COUNT(*) FROM user_settings WHERE user_id = 123456789",
            [],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(settings_count, 1, "User settings should exist");

    println!("✅ Database setup verified - tables and data correct");
}

// =============================================================================
// EXTENSION MENU UI/UX TESTS
// =============================================================================

/// Test show_services_menu renders extensions cards correctly
#[tokio::test]
#[serial]
async fn test_show_services_menu_renders_extension_cards() {
    use doradura::telegram::show_services_menu;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "en");
    test.mock_all_telegram_api().await;

    let lang = doradura::i18n::lang_from_code("en");
    let registry = doradura::extension::ExtensionRegistry::default_registry();

    let result = show_services_menu(
        test.bot(),
        ChatId(123456789),
        teloxide::types::MessageId(42),
        &lang,
        &registry,
    )
    .await;
    assert!(result.is_ok(), "show_services_menu should succeed");

    let requests = test.mock_server.received_requests().await.unwrap();
    let edit_msg = requests
        .iter()
        .find(|r| {
            let path = r.url.path().to_lowercase();
            path.contains("editmessagetext") || path.contains("editmessagecaption")
        })
        .expect("Should have edit message request");

    let body: serde_json::Value = serde_json::from_slice(&edit_msg.body).unwrap();

    // Verify text contains extension header and categories
    let text = body["text"].as_str().or_else(|| body["caption"].as_str()).unwrap_or("");
    assert!(
        text.contains("Extensions") || text.contains("Расширения") || text.contains("🧩"),
        "Should contain extensions header, got: {}",
        &text[..text.len().min(200)]
    ); // "Расширения" is Russian for "Extensions"

    // Verify keyboard has extension buttons
    let keyboard = &body["reply_markup"]["inline_keyboard"];
    assert!(keyboard.is_array(), "Should have inline_keyboard");

    let rows = keyboard.as_array().unwrap();
    // 4 extensions + 1 back button = 5 rows minimum
    assert!(
        rows.len() >= 5,
        "Should have at least 5 button rows (4 extensions + back), got {}",
        rows.len()
    );

    // Verify extension detail buttons have correct callback format
    let mut ext_buttons = Vec::new();
    let mut has_back_button = false;
    for row in rows {
        let btns = row.as_array().unwrap();
        for btn in btns {
            let cb = btn["callback_data"].as_str().unwrap_or("");
            if cb.starts_with("ext:detail:") {
                ext_buttons.push(cb.to_string());
            }
            if cb == "back:enhanced_main" {
                has_back_button = true;
            }
        }
    }

    assert_eq!(
        ext_buttons.len(),
        4,
        "Should have 4 extension detail buttons, got {:?}",
        ext_buttons
    );
    assert!(
        ext_buttons.contains(&"ext:detail:ytdlp".to_string()),
        "Should have ytdlp button"
    );
    assert!(
        ext_buttons.contains(&"ext:detail:http".to_string()),
        "Should have http button"
    );
    assert!(
        ext_buttons.contains(&"ext:detail:converter".to_string()),
        "Should have converter button"
    );
    assert!(
        ext_buttons.contains(&"ext:detail:audio_effects".to_string()),
        "Should have audio_effects button"
    );
    assert!(has_back_button, "Should have back button");

    println!(
        "✅ show_services_menu: {} extension buttons + back, text len={}",
        ext_buttons.len(),
        text.len()
    );
}

/// Test show_services_menu in Russian locale
#[tokio::test]
#[serial]
async fn test_show_services_menu_russian_locale() {
    use doradura::telegram::show_services_menu;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "ru");
    test.mock_all_telegram_api().await;

    let lang = doradura::i18n::lang_from_code("ru");
    let registry = doradura::extension::ExtensionRegistry::default_registry();

    let result = show_services_menu(
        test.bot(),
        ChatId(123456789),
        teloxide::types::MessageId(42),
        &lang,
        &registry,
    )
    .await;
    assert!(result.is_ok(), "show_services_menu should succeed in Russian");

    let requests = test.mock_server.received_requests().await.unwrap();
    let edit_msg = requests
        .iter()
        .find(|r| {
            let path = r.url.path().to_lowercase();
            path.contains("editmessagetext") || path.contains("editmessagecaption")
        })
        .expect("Should have edit message request");

    let body: serde_json::Value = serde_json::from_slice(&edit_msg.body).unwrap();
    let text = body["text"].as_str().or_else(|| body["caption"].as_str()).unwrap_or("");

    // Russian locale should have Russian text ("Расширения" = "Extensions" in Russian)
    assert!(
        text.contains("Расширения") || text.contains("🧩"),
        "Russian locale should contain Russian header"
    );

    println!("✅ show_services_menu (ru): text len={}", text.len());
}

/// Test show_services_menu renders all categories
#[tokio::test]
#[serial]
async fn test_show_services_menu_all_categories_present() {
    use doradura::telegram::show_services_menu;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "en");
    test.mock_all_telegram_api().await;

    let lang = doradura::i18n::lang_from_code("en");
    let registry = doradura::extension::ExtensionRegistry::default_registry();

    let result = show_services_menu(
        test.bot(),
        ChatId(123456789),
        teloxide::types::MessageId(42),
        &lang,
        &registry,
    )
    .await;
    assert!(result.is_ok());

    let requests = test.mock_server.received_requests().await.unwrap();
    let edit_msg = requests
        .iter()
        .find(|r| {
            let path = r.url.path().to_lowercase();
            path.contains("editmessagetext") || path.contains("editmessagecaption")
        })
        .unwrap();

    let body: serde_json::Value = serde_json::from_slice(&edit_msg.body).unwrap();
    let text = body["text"].as_str().or_else(|| body["caption"].as_str()).unwrap_or("");

    // All 3 category icons should be present
    assert!(text.contains("📥"), "Should contain Downloads category icon");
    assert!(text.contains("🔄"), "Should contain Conversion category icon");
    assert!(text.contains("🎛️"), "Should contain Processing category icon");

    // Extension icons should be present
    assert!(text.contains("🌐"), "Should contain yt-dlp icon");
    assert!(text.contains("📥"), "Should contain HTTP downloader icon");

    // Status should be present
    assert!(text.contains("✅"), "Should contain active status indicator");

    println!("✅ show_services_menu: all categories and icons verified");
}

/// Test handle_menu_callback with mode:services
#[tokio::test]
#[serial]
async fn test_callback_mode_services() {
    use doradura::telegram::handle_menu_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "en");
    test.mock_all_telegram_api().await;

    let callback = RealHandlerTest::create_callback_from_json("mode:services", 123456789, 123456789);

    let result = handle_menu_callback(
        test.bot().clone(),
        callback,
        test.deps.db_pool.clone(),
        test.deps.download_queue.clone(),
        test.deps.rate_limiter.clone(),
        test.deps.extension_registry.clone(),
        test.deps.downsub_gateway.clone(),
        test.deps.subtitle_cache.clone(),
    )
    .await;
    assert!(result.is_ok(), "mode:services callback should succeed");

    let requests = test.mock_server.received_requests().await.unwrap();

    // Should have answerCallbackQuery
    let has_answer = requests
        .iter()
        .any(|r| r.url.path().to_lowercase().contains("answercallbackquery"));
    assert!(has_answer, "Should answer callback query");

    // Should have editMessageText with extension content
    let edit_msg = requests.iter().find(|r| {
        let path = r.url.path().to_lowercase();
        path.contains("editmessagetext") || path.contains("editmessagecaption")
    });
    assert!(edit_msg.is_some(), "Should edit message to show services");

    let body: serde_json::Value = serde_json::from_slice(&edit_msg.unwrap().body).unwrap();
    let text = body["text"].as_str().or_else(|| body["caption"].as_str()).unwrap_or("");
    assert!(
        text.contains("🧩") || text.contains("Extensions"),
        "Should show extensions menu"
    );

    println!(
        "✅ mode:services callback: extension menu shown, {} API calls",
        requests.len()
    );
}

/// Test handle_menu_callback with main:services
#[tokio::test]
#[serial]
async fn test_callback_main_services() {
    use doradura::telegram::handle_menu_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "en");
    test.mock_all_telegram_api().await;

    let callback = RealHandlerTest::create_callback_from_json("main:services", 123456789, 123456789);

    let result = handle_menu_callback(
        test.bot().clone(),
        callback,
        test.deps.db_pool.clone(),
        test.deps.download_queue.clone(),
        test.deps.rate_limiter.clone(),
        test.deps.extension_registry.clone(),
        test.deps.downsub_gateway.clone(),
        test.deps.subtitle_cache.clone(),
    )
    .await;
    assert!(result.is_ok(), "main:services callback should succeed");

    let requests = test.mock_server.received_requests().await.unwrap();
    let edit_msg = requests.iter().find(|r| {
        let path = r.url.path().to_lowercase();
        path.contains("editmessagetext") || path.contains("editmessagecaption")
    });
    assert!(
        edit_msg.is_some(),
        "Should edit message to show services from main menu"
    );

    println!(
        "✅ main:services callback: extension menu shown, {} API calls",
        requests.len()
    );
}

/// Test handle_menu_callback with ext:detail:ytdlp
#[tokio::test]
#[serial]
async fn test_callback_ext_detail_ytdlp() {
    use doradura::telegram::handle_menu_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "en");
    test.mock_all_telegram_api().await;

    let callback = RealHandlerTest::create_callback_from_json("ext:detail:ytdlp", 123456789, 123456789);

    let result = handle_menu_callback(
        test.bot().clone(),
        callback,
        test.deps.db_pool.clone(),
        test.deps.download_queue.clone(),
        test.deps.rate_limiter.clone(),
        test.deps.extension_registry.clone(),
        test.deps.downsub_gateway.clone(),
        test.deps.subtitle_cache.clone(),
    )
    .await;
    assert!(result.is_ok(), "ext:detail:ytdlp callback should succeed");

    let requests = test.mock_server.received_requests().await.unwrap();
    let edit_msg = requests.iter().find(|r| {
        let path = r.url.path().to_lowercase();
        path.contains("editmessagetext") || path.contains("editmessagecaption")
    });
    assert!(edit_msg.is_some(), "Should edit message to show ytdlp detail");

    let body: serde_json::Value = serde_json::from_slice(&edit_msg.unwrap().body).unwrap();
    let text = body["text"].as_str().or_else(|| body["caption"].as_str()).unwrap_or("");

    // Should contain ytdlp extension details
    assert!(text.contains("🌐"), "Should contain ytdlp icon");
    assert!(
        text.contains("YouTube") || text.contains("TikTok"),
        "Should contain ytdlp capabilities, got: {}",
        &text[..text.len().min(300)]
    );
    assert!(text.contains("✅"), "Should contain active status");

    // Verify back button
    let keyboard = &body["reply_markup"]["inline_keyboard"];
    let rows = keyboard.as_array().unwrap();
    let has_back = rows.iter().any(|row| {
        row.as_array()
            .unwrap()
            .iter()
            .any(|btn| btn["callback_data"].as_str().unwrap_or("") == "ext:back")
    });
    assert!(has_back, "Should have ext:back button");

    println!(
        "✅ ext:detail:ytdlp: showed {} chars with capabilities and back button",
        text.len()
    );
}

/// Test handle_menu_callback with ext:detail:converter
#[tokio::test]
#[serial]
async fn test_callback_ext_detail_converter() {
    use doradura::telegram::handle_menu_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "en");
    test.mock_all_telegram_api().await;

    let callback = RealHandlerTest::create_callback_from_json("ext:detail:converter", 123456789, 123456789);

    let result = handle_menu_callback(
        test.bot().clone(),
        callback,
        test.deps.db_pool.clone(),
        test.deps.download_queue.clone(),
        test.deps.rate_limiter.clone(),
        test.deps.extension_registry.clone(),
        test.deps.downsub_gateway.clone(),
        test.deps.subtitle_cache.clone(),
    )
    .await;
    assert!(result.is_ok(), "ext:detail:converter callback should succeed");

    let requests = test.mock_server.received_requests().await.unwrap();
    let edit_msg = requests.iter().find(|r| {
        let path = r.url.path().to_lowercase();
        path.contains("editmessagetext") || path.contains("editmessagecaption")
    });
    assert!(edit_msg.is_some(), "Should show converter detail");

    let body: serde_json::Value = serde_json::from_slice(&edit_msg.unwrap().body).unwrap();
    let text = body["text"].as_str().or_else(|| body["caption"].as_str()).unwrap_or("");

    assert!(text.contains("🔄"), "Should contain converter icon");
    assert!(
        text.contains("GIF") || text.contains("Compress"),
        "Should contain converter capabilities"
    );

    println!("✅ ext:detail:converter: showed capabilities");
}

/// Test handle_menu_callback with ext:detail:audio_effects
#[tokio::test]
#[serial]
async fn test_callback_ext_detail_audio_effects() {
    use doradura::telegram::handle_menu_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "en");
    test.mock_all_telegram_api().await;

    let callback = RealHandlerTest::create_callback_from_json("ext:detail:audio_effects", 123456789, 123456789);

    let result = handle_menu_callback(
        test.bot().clone(),
        callback,
        test.deps.db_pool.clone(),
        test.deps.download_queue.clone(),
        test.deps.rate_limiter.clone(),
        test.deps.extension_registry.clone(),
        test.deps.downsub_gateway.clone(),
        test.deps.subtitle_cache.clone(),
    )
    .await;
    assert!(result.is_ok(), "ext:detail:audio_effects callback should succeed");

    let requests = test.mock_server.received_requests().await.unwrap();
    let edit_msg = requests.iter().find(|r| {
        let path = r.url.path().to_lowercase();
        path.contains("editmessagetext") || path.contains("editmessagecaption")
    });
    assert!(edit_msg.is_some(), "Should show audio effects detail");

    let body: serde_json::Value = serde_json::from_slice(&edit_msg.unwrap().body).unwrap();
    let text = body["text"].as_str().or_else(|| body["caption"].as_str()).unwrap_or("");

    assert!(
        text.contains("Pitch") || text.contains("Tempo") || text.contains("Bass"),
        "Should contain audio effects capabilities"
    );

    println!("✅ ext:detail:audio_effects: showed capabilities");
}

/// Test handle_menu_callback with ext:detail:http
#[tokio::test]
#[serial]
async fn test_callback_ext_detail_http() {
    use doradura::telegram::handle_menu_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "en");
    test.mock_all_telegram_api().await;

    let callback = RealHandlerTest::create_callback_from_json("ext:detail:http", 123456789, 123456789);

    let result = handle_menu_callback(
        test.bot().clone(),
        callback,
        test.deps.db_pool.clone(),
        test.deps.download_queue.clone(),
        test.deps.rate_limiter.clone(),
        test.deps.extension_registry.clone(),
        test.deps.downsub_gateway.clone(),
        test.deps.subtitle_cache.clone(),
    )
    .await;
    assert!(result.is_ok(), "ext:detail:http callback should succeed");

    let requests = test.mock_server.received_requests().await.unwrap();
    let edit_msg = requests.iter().find(|r| {
        let path = r.url.path().to_lowercase();
        path.contains("editmessagetext") || path.contains("editmessagecaption")
    });
    assert!(edit_msg.is_some(), "Should show http downloader detail");

    let body: serde_json::Value = serde_json::from_slice(&edit_msg.unwrap().body).unwrap();
    let text = body["text"].as_str().or_else(|| body["caption"].as_str()).unwrap_or("");

    assert!(text.contains("📥"), "Should contain http downloader icon");
    assert!(
        text.contains("MP3") || text.contains("MP4") || text.contains("Resume"),
        "Should contain http capabilities"
    );

    println!("✅ ext:detail:http: showed capabilities");
}

/// Test ext:back returns to extensions menu
#[tokio::test]
#[serial]
async fn test_callback_ext_back() {
    use doradura::telegram::handle_menu_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "en");
    test.mock_all_telegram_api().await;

    let callback = RealHandlerTest::create_callback_from_json("ext:back", 123456789, 123456789);

    let result = handle_menu_callback(
        test.bot().clone(),
        callback,
        test.deps.db_pool.clone(),
        test.deps.download_queue.clone(),
        test.deps.rate_limiter.clone(),
        test.deps.extension_registry.clone(),
        test.deps.downsub_gateway.clone(),
        test.deps.subtitle_cache.clone(),
    )
    .await;
    assert!(result.is_ok(), "ext:back callback should succeed");

    let requests = test.mock_server.received_requests().await.unwrap();
    let edit_msg = requests.iter().find(|r| {
        let path = r.url.path().to_lowercase();
        path.contains("editmessagetext") || path.contains("editmessagecaption")
    });
    assert!(edit_msg.is_some(), "Should edit message back to extensions list");

    let body: serde_json::Value = serde_json::from_slice(&edit_msg.unwrap().body).unwrap();
    let text = body["text"].as_str().or_else(|| body["caption"].as_str()).unwrap_or("");

    // Should show extensions list (not detail)
    // "Расширения" is Russian for "Extensions"
    assert!(
        text.contains("🧩") || text.contains("Extensions") || text.contains("Расширения"),
        "ext:back should return to extensions list"
    );

    // Should have ext:detail buttons (extensions list, not detail page)
    let keyboard = &body["reply_markup"]["inline_keyboard"];
    let rows = keyboard.as_array().unwrap();
    let ext_detail_count = rows
        .iter()
        .flat_map(|row| row.as_array().unwrap())
        .filter(|btn| btn["callback_data"].as_str().unwrap_or("").starts_with("ext:detail:"))
        .count();
    assert!(
        ext_detail_count >= 4,
        "Should have extension detail buttons after going back"
    );

    println!(
        "✅ ext:back: returned to extensions list with {} detail buttons",
        ext_detail_count
    );
}

/// Test ext:detail with nonexistent extension
#[tokio::test]
#[serial]
async fn test_callback_ext_detail_nonexistent() {
    use doradura::telegram::handle_menu_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "en");
    test.mock_all_telegram_api().await;

    let callback = RealHandlerTest::create_callback_from_json("ext:detail:nonexistent", 123456789, 123456789);

    let result = handle_menu_callback(
        test.bot().clone(),
        callback,
        test.deps.db_pool.clone(),
        test.deps.download_queue.clone(),
        test.deps.rate_limiter.clone(),
        test.deps.extension_registry.clone(),
        test.deps.downsub_gateway.clone(),
        test.deps.subtitle_cache.clone(),
    )
    .await;
    // Should not crash, just silently return Ok
    assert!(result.is_ok(), "ext:detail with nonexistent ID should not crash");

    println!("✅ ext:detail:nonexistent: handled gracefully without crash");
}

/// Test services menu button in enhanced main menu has correct callback
#[tokio::test]
#[serial]
async fn test_enhanced_menu_has_services_button() {
    use doradura::telegram::show_enhanced_main_menu;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "en");
    test.mock_all_telegram_api().await;

    let result = show_enhanced_main_menu(test.bot(), ChatId(123456789), test.deps.db_pool.clone()).await;
    assert!(result.is_ok());

    let requests = test.mock_server.received_requests().await.unwrap();
    let send_msg = requests
        .iter()
        .find(|r| r.url.path().to_lowercase().contains("sendmessage"))
        .expect("Should have sendMessage");

    let body: serde_json::Value = serde_json::from_slice(&send_msg.body).unwrap();
    let keyboard = &body["reply_markup"]["inline_keyboard"];
    let rows = keyboard.as_array().unwrap();

    // Find the services/extensions button
    let has_services_button = rows.iter().any(|row| {
        row.as_array().unwrap().iter().any(|btn| {
            let cb = btn["callback_data"].as_str().unwrap_or("");
            let text = btn["text"].as_str().unwrap_or("");
            cb == "main:services" && text.contains("🧩")
        })
    });
    assert!(
        has_services_button,
        "Enhanced menu should have 🧩 Extensions button with main:services callback"
    );

    println!("✅ Enhanced menu has services/extensions button with correct callback");
}

/// Test all 4 locales render show_services_menu without errors
#[tokio::test]
#[serial]
async fn test_show_services_menu_all_locales() {
    use doradura::telegram::show_services_menu;

    let locales = ["en", "ru", "fr", "de"];

    for locale in &locales {
        let test = RealHandlerTest::new().await;
        test.create_test_user(123456789, "testuser", locale);
        test.mock_all_telegram_api().await;

        let lang = doradura::i18n::lang_from_code(locale);
        let registry = doradura::extension::ExtensionRegistry::default_registry();

        let result = show_services_menu(
            test.bot(),
            ChatId(123456789),
            teloxide::types::MessageId(42),
            &lang,
            &registry,
        )
        .await;
        assert!(
            result.is_ok(),
            "show_services_menu should succeed for locale '{}'",
            locale
        );

        let requests = test.mock_server.received_requests().await.unwrap();
        let edit_msg = requests.iter().find(|r| {
            let path = r.url.path().to_lowercase();
            path.contains("editmessagetext") || path.contains("editmessagecaption")
        });
        assert!(edit_msg.is_some(), "Should edit message for locale '{}'", locale);

        let body: serde_json::Value = serde_json::from_slice(&edit_msg.unwrap().body).unwrap();
        let text = body["text"].as_str().or_else(|| body["caption"].as_str()).unwrap_or("");
        assert!(!text.is_empty(), "Text should not be empty for locale '{}'", locale);
        assert!(
            text.contains("🧩"),
            "Should have extensions header emoji for locale '{}'",
            locale
        );

        println!("✅ show_services_menu ({}): text len={}", locale, text.len());
    }
}

// =============================================================================
// PHASE 1: VIDEO CONVERTER CALLBACK TESTS
// =============================================================================

/// Test handle_videos_callback with videos:convert:audio route (Phase 1 wiring)
#[tokio::test]
#[serial]
async fn test_videos_convert_audio_callback_routing() {
    use doradura::telegram::handle_videos_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "ru");
    let upload_id = test.create_test_upload(123456789, "test_video.mp4", "video", "file_id_123");
    test.mock_all_telegram_api().await;

    let result = handle_videos_callback(
        test.bot(),
        "cb_123".into(),
        ChatId(123456789),
        teloxide::types::MessageId(42),
        &format!("videos:convert:audio:{}", upload_id),
        test.deps.db_pool.clone(),
    )
    .await;
    assert!(
        result.is_ok(),
        "videos:convert:audio should not crash: {:?}",
        result.err()
    );

    // Verify status message was sent (extraction in progress)
    let requests = test.mock_server.received_requests().await.unwrap();
    let has_answer_cb = requests
        .iter()
        .any(|r| r.url.path().to_lowercase().contains("answercallbackquery"));
    assert!(has_answer_cb, "Should answer callback query");

    // Should attempt to delete the original message and send status
    let send_msgs: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path().to_lowercase().contains("sendmessage"))
        .collect();
    assert!(!send_msgs.is_empty(), "Should send status message for audio extraction");

    // Verify at least one message contains audio extraction text
    let has_audio_status = send_msgs.iter().any(|r| {
        let body = String::from_utf8_lossy(&r.body);
        // "аудио" = "audio", "Извлекаю" = "Extracting" in Russian
        body.contains("аудио") || body.contains("audio") || body.contains("Извлекаю")
    });
    assert!(has_audio_status, "Should send audio extraction status message");

    println!(
        "✅ videos:convert:audio:{} routed correctly with status message",
        upload_id
    );
}

/// Test handle_videos_callback with videos:convert:gif route (Phase 1 wiring)
#[tokio::test]
#[serial]
async fn test_videos_convert_gif_callback_routing() {
    use doradura::telegram::handle_videos_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "ru");
    let upload_id = test.create_test_upload(123456789, "test_video.mp4", "video", "file_id_456");
    test.mock_all_telegram_api().await;

    let result = handle_videos_callback(
        test.bot(),
        "cb_456".into(),
        ChatId(123456789),
        teloxide::types::MessageId(42),
        &format!("videos:convert:gif:{}", upload_id),
        test.deps.db_pool.clone(),
    )
    .await;
    assert!(
        result.is_ok(),
        "videos:convert:gif should not crash: {:?}",
        result.err()
    );

    let requests = test.mock_server.received_requests().await.unwrap();
    let send_msgs: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path().to_lowercase().contains("sendmessage"))
        .collect();
    assert!(!send_msgs.is_empty(), "Should send status message for GIF creation");

    let has_gif_status = send_msgs.iter().any(|r| {
        let body = String::from_utf8_lossy(&r.body);
        body.contains("GIF") || body.contains("gif")
    });
    assert!(has_gif_status, "Should send GIF creation status message");

    println!("✅ videos:convert:gif:{} routed correctly", upload_id);
}

/// Test handle_videos_callback with videos:convert:compress route (Phase 1 wiring)
#[tokio::test]
#[serial]
async fn test_videos_convert_compress_callback_routing() {
    use doradura::telegram::handle_videos_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "ru");
    let upload_id = test.create_test_upload(123456789, "test_video.mp4", "video", "file_id_789");
    test.mock_all_telegram_api().await;

    let result = handle_videos_callback(
        test.bot(),
        "cb_789".into(),
        ChatId(123456789),
        teloxide::types::MessageId(42),
        &format!("videos:convert:compress:{}", upload_id),
        test.deps.db_pool.clone(),
    )
    .await;
    assert!(
        result.is_ok(),
        "videos:convert:compress should not crash: {:?}",
        result.err()
    );

    let requests = test.mock_server.received_requests().await.unwrap();
    let send_msgs: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path().to_lowercase().contains("sendmessage"))
        .collect();
    assert!(!send_msgs.is_empty(), "Should send status message for compression");

    let has_compress_status = send_msgs.iter().any(|r| {
        let body = String::from_utf8_lossy(&r.body);
        body.contains("Compressing") || body.contains("compress")
    });
    assert!(has_compress_status, "Should send compression status message");

    println!("✅ videos:convert:compress:{} routed correctly", upload_id);
}

/// Test handle_videos_callback with convert route when upload doesn't exist
#[tokio::test]
#[serial]
async fn test_videos_convert_nonexistent_upload() {
    use doradura::telegram::handle_videos_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "ru");
    test.mock_all_telegram_api().await;

    // Use non-existent upload ID
    let result = handle_videos_callback(
        test.bot(),
        "cb_000".into(),
        ChatId(123456789),
        teloxide::types::MessageId(42),
        "videos:convert:audio:99999",
        test.deps.db_pool.clone(),
    )
    .await;
    assert!(result.is_ok(), "Should handle non-existent upload gracefully");

    println!("✅ videos:convert with non-existent upload handled gracefully");
}

/// Test handle_videos_callback with cancel/close action
#[tokio::test]
#[serial]
async fn test_videos_cancel_callback() {
    use doradura::telegram::handle_videos_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "ru");
    test.mock_all_telegram_api().await;

    let result = handle_videos_callback(
        test.bot(),
        "cb_cancel".into(),
        ChatId(123456789),
        teloxide::types::MessageId(42),
        "videos:cancel",
        test.deps.db_pool.clone(),
    )
    .await;
    assert!(result.is_ok(), "videos:cancel should succeed: {:?}", result.err());

    let requests = test.mock_server.received_requests().await.unwrap();
    let has_delete = requests
        .iter()
        .any(|r| r.url.path().to_lowercase().contains("deletemessage"));
    assert!(has_delete, "Should delete message on cancel");

    println!("✅ videos:cancel deletes message");
}

/// Test handle_videos_callback with unknown action
#[tokio::test]
#[serial]
async fn test_videos_unknown_action() {
    use doradura::telegram::handle_videos_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "ru");
    test.mock_all_telegram_api().await;

    let result = handle_videos_callback(
        test.bot(),
        "cb_unk".into(),
        ChatId(123456789),
        teloxide::types::MessageId(42),
        "videos:unknown_action",
        test.deps.db_pool.clone(),
    )
    .await;
    assert!(result.is_ok(), "Unknown action should not crash");

    println!("✅ videos:unknown_action handled without crash");
}

/// Test handle_videos_callback with malformed data
#[tokio::test]
#[serial]
async fn test_videos_malformed_data() {
    use doradura::telegram::handle_videos_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "ru");
    test.mock_all_telegram_api().await;

    // Too few parts
    let result = handle_videos_callback(
        test.bot(),
        "cb_bad".into(),
        ChatId(123456789),
        teloxide::types::MessageId(42),
        "videos",
        test.deps.db_pool.clone(),
    )
    .await;
    assert!(result.is_ok(), "Malformed data should not crash");

    println!("✅ Malformed callback data handled gracefully");
}

// =============================================================================
// SHOW VIDEOS PAGE TESTS
// =============================================================================

/// Test show_videos_page with no uploads
#[tokio::test]
#[serial]
async fn test_show_videos_page_empty() {
    use doradura::telegram::show_videos_page;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "ru");
    test.mock_all_telegram_api().await;

    let result = show_videos_page(test.bot(), ChatId(123456789), test.deps.db_pool.clone(), 0, None, None).await;
    assert!(result.is_ok(), "show_videos_page should succeed with no uploads");

    let requests = test.mock_server.received_requests().await.unwrap();
    let send_msgs: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path().to_lowercase().contains("sendmessage"))
        .collect();
    assert!(!send_msgs.is_empty(), "Should send a message even with no uploads");

    println!("✅ show_videos_page: empty uploads handled");
}

/// Test show_videos_page with uploads present
#[tokio::test]
#[serial]
async fn test_show_videos_page_with_uploads() {
    use doradura::telegram::show_videos_page;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "ru");
    test.create_test_upload(123456789, "my_video.mp4", "video", "file_vid_1");
    test.create_test_upload(123456789, "my_photo.jpg", "photo", "file_photo_1");
    test.mock_all_telegram_api().await;

    let result = show_videos_page(test.bot(), ChatId(123456789), test.deps.db_pool.clone(), 0, None, None).await;
    assert!(result.is_ok(), "show_videos_page should succeed with uploads");

    let requests = test.mock_server.received_requests().await.unwrap();
    let send_msgs: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path().to_lowercase().contains("sendmessage"))
        .collect();
    assert!(!send_msgs.is_empty(), "Should send a message showing uploads");

    // Verify message contains upload titles
    let has_upload_content = send_msgs.iter().any(|r| {
        let body = String::from_utf8_lossy(&r.body);
        body.contains("my_video") || body.contains("my_photo")
    });
    assert!(has_upload_content, "Should show upload titles in message");

    println!("✅ show_videos_page: uploads displayed");
}

/// Test show_videos_page with filter
#[tokio::test]
#[serial]
async fn test_show_videos_page_with_filter() {
    use doradura::telegram::show_videos_page;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "ru");
    test.create_test_upload(123456789, "filtered_video.mp4", "video", "file_filt_1");
    test.create_test_upload(123456789, "filtered_photo.jpg", "photo", "file_filt_2");
    test.mock_all_telegram_api().await;

    // Filter only videos
    let result = show_videos_page(
        test.bot(),
        ChatId(123456789),
        test.deps.db_pool.clone(),
        0,
        Some("video".to_string()),
        None,
    )
    .await;
    assert!(result.is_ok(), "show_videos_page with filter should succeed");

    println!("✅ show_videos_page: filter works");
}

// =============================================================================
// ENHANCED EXTENSION DETAIL CAPABILITY VERIFICATION
// =============================================================================

/// Test ext:detail:ytdlp shows ALL capabilities
#[tokio::test]
#[serial]
async fn test_ext_detail_ytdlp_all_capabilities() {
    use doradura::telegram::handle_menu_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "en");
    test.mock_all_telegram_api().await;

    let callback = RealHandlerTest::create_callback_from_json("ext:detail:ytdlp", 123456789, 123456789);
    let result = handle_menu_callback(
        test.bot().clone(),
        callback,
        test.deps.db_pool.clone(),
        test.deps.download_queue.clone(),
        test.deps.rate_limiter.clone(),
        test.deps.extension_registry.clone(),
        test.deps.downsub_gateway.clone(),
        test.deps.subtitle_cache.clone(),
    )
    .await;
    assert!(result.is_ok());

    let requests = test.mock_server.received_requests().await.unwrap();
    let edit_msg = requests
        .iter()
        .find(|r| {
            let path = r.url.path().to_lowercase();
            path.contains("editmessagetext") || path.contains("editmessagecaption")
        })
        .unwrap();

    let body: serde_json::Value = serde_json::from_slice(&edit_msg.body).unwrap();
    let text = body["text"].as_str().or_else(|| body["caption"].as_str()).unwrap_or("");

    // Verify ALL 5 ytdlp capabilities
    assert!(text.contains("YouTube"), "Should contain YouTube capability");
    assert!(text.contains("TikTok"), "Should contain TikTok capability");
    assert!(text.contains("Instagram"), "Should contain Instagram capability");
    assert!(text.contains("SoundCloud"), "Should contain SoundCloud capability");
    assert!(text.contains("1000"), "Should contain 1000+ sites capability");
    assert!(text.contains("🌐"), "Should contain ytdlp icon");
    assert!(text.contains("✅"), "Should contain active status");

    // Verify back button
    let keyboard = &body["reply_markup"]["inline_keyboard"];
    let has_back = keyboard.as_array().unwrap().iter().any(|row| {
        row.as_array()
            .unwrap()
            .iter()
            .any(|btn| btn["callback_data"] == "ext:back")
    });
    assert!(has_back, "Should have ext:back button");

    println!("✅ ext:detail:ytdlp: all 5 capabilities verified");
}

/// Test ext:detail:http shows ALL capabilities
#[tokio::test]
#[serial]
async fn test_ext_detail_http_all_capabilities() {
    use doradura::telegram::handle_menu_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "en");
    test.mock_all_telegram_api().await;

    let callback = RealHandlerTest::create_callback_from_json("ext:detail:http", 123456789, 123456789);
    let result = handle_menu_callback(
        test.bot().clone(),
        callback,
        test.deps.db_pool.clone(),
        test.deps.download_queue.clone(),
        test.deps.rate_limiter.clone(),
        test.deps.extension_registry.clone(),
        test.deps.downsub_gateway.clone(),
        test.deps.subtitle_cache.clone(),
    )
    .await;
    assert!(result.is_ok());

    let requests = test.mock_server.received_requests().await.unwrap();
    let edit_msg = requests
        .iter()
        .find(|r| {
            let path = r.url.path().to_lowercase();
            path.contains("editmessagetext") || path.contains("editmessagecaption")
        })
        .unwrap();

    let body: serde_json::Value = serde_json::from_slice(&edit_msg.body).unwrap();
    let text = body["text"].as_str().or_else(|| body["caption"].as_str()).unwrap_or("");

    // Verify ALL 3 http capabilities
    assert!(
        text.contains("MP3") || text.contains("MP4"),
        "Should contain MP3/MP4 capability"
    );
    assert!(
        text.contains("WAV") || text.contains("FLAC"),
        "Should contain WAV/FLAC capability"
    );
    assert!(text.contains("Resume"), "Should contain Resume capability");
    assert!(text.contains("📥"), "Should contain http icon");

    println!("✅ ext:detail:http: all 3 capabilities verified");
}

/// Test ext:detail:converter shows ALL capabilities
#[tokio::test]
#[serial]
async fn test_ext_detail_converter_all_capabilities() {
    use doradura::telegram::handle_menu_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "en");
    test.mock_all_telegram_api().await;

    let callback = RealHandlerTest::create_callback_from_json("ext:detail:converter", 123456789, 123456789);
    let result = handle_menu_callback(
        test.bot().clone(),
        callback,
        test.deps.db_pool.clone(),
        test.deps.download_queue.clone(),
        test.deps.rate_limiter.clone(),
        test.deps.extension_registry.clone(),
        test.deps.downsub_gateway.clone(),
        test.deps.subtitle_cache.clone(),
    )
    .await;
    assert!(result.is_ok());

    let requests = test.mock_server.received_requests().await.unwrap();
    let edit_msg = requests
        .iter()
        .find(|r| {
            let path = r.url.path().to_lowercase();
            path.contains("editmessagetext") || path.contains("editmessagecaption")
        })
        .unwrap();

    let body: serde_json::Value = serde_json::from_slice(&edit_msg.body).unwrap();
    let text = body["text"].as_str().or_else(|| body["caption"].as_str()).unwrap_or("");

    // Verify ALL 5 converter capabilities
    assert!(text.contains("Video Note"), "Should contain Video Note capability");
    assert!(text.contains("GIF"), "Should contain GIF capability");
    assert!(text.contains("MP3 Extract"), "Should contain MP3 Extract capability");
    assert!(text.contains("Compress"), "Should contain Compress capability");
    assert!(text.contains("Documents"), "Should contain Documents capability");
    assert!(text.contains("🔄"), "Should contain converter icon");

    println!("✅ ext:detail:converter: all 5 capabilities verified");
}

/// Test ext:detail:audio_effects shows ALL capabilities
#[tokio::test]
#[serial]
async fn test_ext_detail_audio_effects_all_capabilities() {
    use doradura::telegram::handle_menu_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "en");
    test.mock_all_telegram_api().await;

    let callback = RealHandlerTest::create_callback_from_json("ext:detail:audio_effects", 123456789, 123456789);
    let result = handle_menu_callback(
        test.bot().clone(),
        callback,
        test.deps.db_pool.clone(),
        test.deps.download_queue.clone(),
        test.deps.rate_limiter.clone(),
        test.deps.extension_registry.clone(),
        test.deps.downsub_gateway.clone(),
        test.deps.subtitle_cache.clone(),
    )
    .await;
    assert!(result.is_ok());

    let requests = test.mock_server.received_requests().await.unwrap();
    let edit_msg = requests
        .iter()
        .find(|r| {
            let path = r.url.path().to_lowercase();
            path.contains("editmessagetext") || path.contains("editmessagecaption")
        })
        .unwrap();

    let body: serde_json::Value = serde_json::from_slice(&edit_msg.body).unwrap();
    let text = body["text"].as_str().or_else(|| body["caption"].as_str()).unwrap_or("");

    // Verify ALL 4 audio effects capabilities
    assert!(text.contains("Pitch"), "Should contain Pitch capability");
    assert!(text.contains("Tempo"), "Should contain Tempo capability");
    assert!(text.contains("Bass"), "Should contain Bass Boost capability");
    assert!(text.contains("Morph"), "Should contain Morph capability");
    assert!(text.contains("🎛️"), "Should contain audio effects icon");

    println!("✅ ext:detail:audio_effects: all 4 capabilities verified");
}

// =============================================================================
// EXTENSION DETAIL LOCALE VERIFICATION
// =============================================================================

/// Test ext:detail renders correctly in all 4 locales
#[tokio::test]
#[serial]
async fn test_ext_detail_all_locales() {
    use doradura::telegram::handle_menu_callback;

    let locales_and_expected: &[(&str, &str)] = &[
        ("en", "Media Downloader"),
        ("ru", "Медиа загрузчик"), // Russian: "Media Downloader"
        ("fr", "médias"),
        ("de", "Medien"),
    ];

    for (locale, expected_fragment) in locales_and_expected {
        let test = RealHandlerTest::new().await;
        test.create_test_user(123456789, "testuser", locale);
        test.mock_all_telegram_api().await;

        let callback = RealHandlerTest::create_callback_from_json("ext:detail:ytdlp", 123456789, 123456789);
        let result = handle_menu_callback(
            test.bot().clone(),
            callback,
            test.deps.db_pool.clone(),
            test.deps.download_queue.clone(),
            test.deps.rate_limiter.clone(),
            test.deps.extension_registry.clone(),
            test.deps.downsub_gateway.clone(),
            test.deps.subtitle_cache.clone(),
        )
        .await;
        assert!(
            result.is_ok(),
            "ext:detail:ytdlp should succeed for locale '{}'",
            locale
        );

        let requests = test.mock_server.received_requests().await.unwrap();
        let edit_msg = requests
            .iter()
            .find(|r| {
                let path = r.url.path().to_lowercase();
                path.contains("editmessagetext") || path.contains("editmessagecaption")
            })
            .unwrap();

        let body: serde_json::Value = serde_json::from_slice(&edit_msg.body).unwrap();
        let text = body["text"].as_str().or_else(|| body["caption"].as_str()).unwrap_or("");

        assert!(
            text.contains(expected_fragment),
            "Locale '{}' should contain '{}', got: {}",
            locale,
            expected_fragment,
            &text[..text.len().min(200)]
        );

        // Should NOT contain raw locale keys (e.g., "ext_ytdlp.name")
        assert!(
            !text.contains("ext_ytdlp.name"),
            "Should not contain raw locale key for '{}'",
            locale
        );
        assert!(
            !text.contains("ext_ytdlp.description"),
            "Should not contain raw locale key for '{}'",
            locale
        );

        println!("✅ ext:detail:ytdlp ({}): localized correctly", locale);
    }
}

// =============================================================================
// VIDEOS.RS UNIT TEST EXPANSION
// =============================================================================

/// Test handle_videos_callback with page navigation
#[tokio::test]
#[serial]
async fn test_videos_page_callback() {
    use doradura::telegram::handle_videos_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "ru");
    test.create_test_upload(123456789, "page_video.mp4", "video", "file_page_1");
    test.mock_all_telegram_api().await;

    // Page navigation: videos:page:0:all:123456789
    let result = handle_videos_callback(
        test.bot(),
        "cb_page".into(),
        ChatId(123456789),
        teloxide::types::MessageId(42),
        "videos:page:0:all:123456789",
        test.deps.db_pool.clone(),
    )
    .await;
    assert!(result.is_ok(), "Page navigation should succeed");

    println!("✅ videos:page callback works");
}

/// Test handle_videos_callback with delete action
#[tokio::test]
#[serial]
async fn test_videos_delete_callback() {
    use doradura::telegram::handle_videos_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "ru");
    let upload_id = test.create_test_upload(123456789, "to_delete.mp4", "video", "file_del_1");
    test.mock_all_telegram_api().await;

    let result = handle_videos_callback(
        test.bot(),
        "cb_del".into(),
        ChatId(123456789),
        teloxide::types::MessageId(42),
        &format!("videos:delete:{}", upload_id),
        test.deps.db_pool.clone(),
    )
    .await;
    assert!(result.is_ok(), "Delete action should succeed");

    println!("✅ videos:delete:{} callback works", upload_id);
}

/// Test handle_videos_callback with send actions
#[tokio::test]
#[serial]
async fn test_videos_send_callback() {
    use doradura::telegram::handle_videos_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "ru");
    let upload_id = test.create_test_upload(123456789, "to_send.mp4", "video", "file_send_1");
    test.mock_all_telegram_api().await;

    // Test send as video
    let result = handle_videos_callback(
        test.bot(),
        "cb_send".into(),
        ChatId(123456789),
        teloxide::types::MessageId(42),
        &format!("videos:send:video:{}", upload_id),
        test.deps.db_pool.clone(),
    )
    .await;
    assert!(result.is_ok(), "Send video action should succeed");

    println!("✅ videos:send:video callback works");
}

/// Test handle_videos_callback open action for viewing upload detail (Level 1 menu)
#[tokio::test]
#[serial]
async fn test_videos_open_callback() {
    use doradura::telegram::handle_videos_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "ru");
    let upload_id = test.create_test_upload(123456789, "open_video.mp4", "video", "file_open_1");
    test.mock_all_telegram_api().await;

    let result = handle_videos_callback(
        test.bot(),
        "cb_open".into(),
        ChatId(123456789),
        teloxide::types::MessageId(42),
        &format!("videos:open:{}", upload_id),
        test.deps.db_pool.clone(),
    )
    .await;
    assert!(result.is_ok(), "Open action should succeed");

    // Open now tries editMessageText first (for back-navigation), falls back to sendMessage
    let requests = test.mock_server.received_requests().await.unwrap();

    // Find editMessageText or sendMessage (whichever was used)
    let edit_msgs: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path().to_lowercase().contains("editmessagetext"))
        .collect();
    let send_msgs: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path().to_lowercase().contains("sendmessage"))
        .collect();

    // Should have used either edit or send
    assert!(
        !edit_msgs.is_empty() || !send_msgs.is_empty(),
        "Should edit or send message to show upload details"
    );

    // Get the message body (prefer edit, fall back to send)
    let msg = if !edit_msgs.is_empty() {
        edit_msgs.last().unwrap()
    } else {
        send_msgs.last().unwrap()
    };

    let body: serde_json::Value = serde_json::from_slice(&msg.body).unwrap();
    let keyboard = &body["reply_markup"]["inline_keyboard"];
    if keyboard.is_array() {
        let all_callbacks: Vec<String> = keyboard
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|row| {
                row.as_array()
                    .unwrap()
                    .iter()
                    .map(|btn| btn["callback_data"].as_str().unwrap_or("").to_string())
            })
            .collect();

        // Level 1: Video should have Send + Convert category buttons (not individual convert buttons)
        let has_send_submenu = all_callbacks.iter().any(|cb| cb.contains("submenu:send"));
        let has_convert_submenu = all_callbacks.iter().any(|cb| cb.contains("submenu:convert"));
        let has_delete = all_callbacks.iter().any(|cb| cb.contains("delete:"));

        assert!(
            has_send_submenu,
            "Video Level 1 should have send submenu button, got: {:?}",
            all_callbacks
        );
        assert!(has_convert_submenu, "Video Level 1 should have convert submenu button");
        assert!(has_delete, "Video Level 1 should have delete button");

        println!(
            "✅ videos:open:{} showed Level 1 menu with Send + Convert categories",
            upload_id
        );
    }
}

/// Test show_services_menu has proper MarkdownV2 escaping
#[tokio::test]
#[serial]
async fn test_services_menu_markdown_escaping() {
    use doradura::telegram::show_services_menu;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "en");
    test.mock_all_telegram_api().await;

    let lang = doradura::i18n::lang_from_code("en");
    let registry = doradura::extension::ExtensionRegistry::default_registry();

    let result = show_services_menu(
        test.bot(),
        ChatId(123456789),
        teloxide::types::MessageId(42),
        &lang,
        &registry,
    )
    .await;
    assert!(result.is_ok());

    let requests = test.mock_server.received_requests().await.unwrap();
    let edit_msg = requests
        .iter()
        .find(|r| {
            let path = r.url.path().to_lowercase();
            path.contains("editmessagetext") || path.contains("editmessagecaption")
        })
        .unwrap();

    let body: serde_json::Value = serde_json::from_slice(&edit_msg.body).unwrap();
    let text = body["text"].as_str().or_else(|| body["caption"].as_str()).unwrap_or("");

    // Text should not be empty or contain raw "extensions.header" key
    assert!(!text.is_empty(), "Menu text should not be empty");
    assert!(!text.contains("extensions.header"), "Should not contain raw locale key");
    assert!(
        !text.contains("extensions.category"),
        "Should not contain raw category key"
    );
    assert!(!text.contains("extensions.status"), "Should not contain raw status key");
    assert!(!text.contains("extensions.footer"), "Should not contain raw footer key");

    println!("✅ Services menu: no raw locale keys, proper rendering");
}

/// Test submenu:send callback edits message with send options
#[tokio::test]
#[serial]
async fn test_videos_submenu_send_edits_message() {
    use doradura::telegram::handle_videos_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "ru");
    let upload_id = test.create_test_upload(123456789, "submenu_video.mp4", "video", "file_sub_1");
    test.mock_all_telegram_api().await;

    let result = handle_videos_callback(
        test.bot(),
        "cb_sub_send".into(),
        ChatId(123456789),
        teloxide::types::MessageId(42),
        &format!("videos:submenu:send:{}", upload_id),
        test.deps.db_pool.clone(),
    )
    .await;
    assert!(result.is_ok(), "Submenu send should succeed");

    let requests = test.mock_server.received_requests().await.unwrap();
    let edit_msgs: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path().to_lowercase().contains("editmessagetext"))
        .collect();

    assert!(!edit_msgs.is_empty(), "Submenu send should use editMessageText");

    let body: serde_json::Value = serde_json::from_slice(&edit_msgs.last().unwrap().body).unwrap();
    let keyboard = &body["reply_markup"]["inline_keyboard"];
    assert!(keyboard.is_array(), "Should have keyboard");

    let all_callbacks: Vec<String> = keyboard
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|row| {
            row.as_array()
                .unwrap()
                .iter()
                .map(|btn| btn["callback_data"].as_str().unwrap_or("").to_string())
        })
        .collect();

    // Should have send:video and send:document for video uploads
    assert!(
        all_callbacks.iter().any(|cb| cb.contains("send:video")),
        "Should have send:video"
    );
    assert!(
        all_callbacks.iter().any(|cb| cb.contains("send:document")),
        "Should have send:document"
    );
    // Should have back button
    assert!(
        all_callbacks.iter().any(|cb| cb.contains("videos:open:")),
        "Should have back button"
    );

    println!("✅ submenu:send showed send options with back button");
}

/// Test submenu:convert callback edits message with conversion options
#[tokio::test]
#[serial]
async fn test_videos_submenu_convert_edits_message() {
    use doradura::telegram::handle_videos_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "ru");
    let upload_id = test.create_test_upload(123456789, "conv_video.mp4", "video", "file_conv_1");
    test.mock_all_telegram_api().await;

    let result = handle_videos_callback(
        test.bot(),
        "cb_sub_conv".into(),
        ChatId(123456789),
        teloxide::types::MessageId(42),
        &format!("videos:submenu:convert:{}", upload_id),
        test.deps.db_pool.clone(),
    )
    .await;
    assert!(result.is_ok(), "Submenu convert should succeed");

    let requests = test.mock_server.received_requests().await.unwrap();
    let edit_msgs: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path().to_lowercase().contains("editmessagetext"))
        .collect();

    assert!(!edit_msgs.is_empty(), "Submenu convert should use editMessageText");

    let body: serde_json::Value = serde_json::from_slice(&edit_msgs.last().unwrap().body).unwrap();
    let keyboard = &body["reply_markup"]["inline_keyboard"];
    assert!(keyboard.is_array());

    let all_callbacks: Vec<String> = keyboard
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|row| {
            row.as_array()
                .unwrap()
                .iter()
                .map(|btn| btn["callback_data"].as_str().unwrap_or("").to_string())
        })
        .collect();

    // Should have all 4 conversion options
    assert!(
        all_callbacks.iter().any(|cb| cb.contains("convert:circle")),
        "Should have circle"
    );
    assert!(
        all_callbacks.iter().any(|cb| cb.contains("convert:audio")),
        "Should have MP3"
    );
    assert!(
        all_callbacks.iter().any(|cb| cb.contains("convert:gif")),
        "Should have GIF"
    );
    assert!(
        all_callbacks.iter().any(|cb| cb.contains("convert:compress")),
        "Should have compress"
    );
    assert!(
        all_callbacks.iter().any(|cb| cb.contains("videos:open:")),
        "Should have back button"
    );

    println!("✅ submenu:convert showed all 4 conversion options with back button");
}

/// Test submenu with deleted/non-existent upload shows error
#[tokio::test]
#[serial]
async fn test_videos_submenu_deleted_upload() {
    use doradura::telegram::handle_videos_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "ru");
    test.mock_all_telegram_api().await;

    // Use a non-existent upload ID
    let result = handle_videos_callback(
        test.bot(),
        "cb_sub_gone".into(),
        ChatId(123456789),
        teloxide::types::MessageId(42),
        "videos:submenu:send:99999",
        test.deps.db_pool.clone(),
    )
    .await;
    assert!(result.is_ok(), "Should not error on missing upload");

    let requests = test.mock_server.received_requests().await.unwrap();
    let edit_msgs: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path().to_lowercase().contains("editmessagetext"))
        .collect();

    assert!(!edit_msgs.is_empty(), "Should edit message with error");

    let body: serde_json::Value = serde_json::from_slice(&edit_msgs.last().unwrap().body).unwrap();
    let text = body["text"].as_str().unwrap_or("");
    assert!(
        text.contains("не найден") || text.contains("not found"), // "не найден" = "not found" in Russian
        "Should show file-not-found error, got: {}",
        text
    );

    println!("✅ submenu with deleted upload showed error message");
}

/// Test back navigation from submenu edits back to Level 1
#[tokio::test]
#[serial]
async fn test_videos_back_navigation() {
    use doradura::telegram::handle_videos_callback;

    let test = RealHandlerTest::new().await;
    test.create_test_user(123456789, "testuser", "ru");
    let upload_id = test.create_test_upload(123456789, "back_video.mp4", "video", "file_back_1");
    test.mock_all_telegram_api().await;

    // Simulate clicking "Back" from submenu (uses videos:open:{id} which should edit the message)
    let result = handle_videos_callback(
        test.bot(),
        "cb_back".into(),
        ChatId(123456789),
        teloxide::types::MessageId(42),
        &format!("videos:open:{}", upload_id),
        test.deps.db_pool.clone(),
    )
    .await;
    assert!(result.is_ok(), "Back navigation should succeed");

    let requests = test.mock_server.received_requests().await.unwrap();
    let edit_msgs: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path().to_lowercase().contains("editmessagetext"))
        .collect();

    // Since mock returns success for editMessageText, it should use edit (not send+delete)
    assert!(!edit_msgs.is_empty(), "Back navigation should use editMessageText");

    let body: serde_json::Value = serde_json::from_slice(&edit_msgs.last().unwrap().body).unwrap();
    let keyboard = &body["reply_markup"]["inline_keyboard"];
    assert!(keyboard.is_array());

    let all_callbacks: Vec<String> = keyboard
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|row| {
            row.as_array()
                .unwrap()
                .iter()
                .map(|btn| btn["callback_data"].as_str().unwrap_or("").to_string())
        })
        .collect();

    // Should be back to Level 1 with category buttons
    assert!(
        all_callbacks.iter().any(|cb| cb.contains("submenu:send")),
        "Back should show Level 1 with send submenu"
    );
    assert!(
        all_callbacks.iter().any(|cb| cb.contains("submenu:convert")),
        "Back should show Level 1 with convert submenu"
    );

    println!("✅ Back navigation returned to Level 1 menu");
}
