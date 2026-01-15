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

        // Create in-memory database
        let db_pool = Arc::new(create_pool(":memory:").expect("Failed to create test database"));

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
                "#,
            )
            .expect("Failed to create tables");
        }

        let download_queue = Arc::new(DownloadQueue::new());
        let rate_limiter = Arc::new(RateLimiter::new());
        let downsub_gateway = Arc::new(DownsubGateway::from_env().await);

        let deps = HandlerDeps::new(
            db_pool,
            download_queue,
            rate_limiter,
            downsub_gateway,
            Some("test_bot".to_string()),
            UserId(987654321),
        );

        Self { mock_server, bot, deps }
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
            .and(path_regex("/bot[^/]+/sendMessage"))
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
            .and(path_regex("/bot[^/]+/editMessageText"))
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
            .and(path_regex("/bot[^/]+/answerCallbackQuery"))
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
            .and(path_regex("/bot[^/]+/setMyCommands"))
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
            .and(path_regex("/bot[^/]+/sendVoice"))
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
            .and(path_regex("/bot[^/]+/sendMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(send_msg.clone()))
            .mount(&self.mock_server)
            .await;

        // editMessageText
        Mock::given(method("POST"))
            .and(path_regex("/bot[^/]+/editMessageText"))
            .respond_with(ResponseTemplate::new(200).set_body_json(send_msg.clone()))
            .mount(&self.mock_server)
            .await;

        // editMessageReplyMarkup
        Mock::given(method("POST"))
            .and(path_regex("/bot[^/]+/editMessageReplyMarkup"))
            .respond_with(ResponseTemplate::new(200).set_body_json(send_msg.clone()))
            .mount(&self.mock_server)
            .await;

        // answerCallbackQuery
        let answer_cb = serde_json::json!({ "ok": true, "result": true });
        Mock::given(method("POST"))
            .and(path_regex("/bot[^/]+/answerCallbackQuery"))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer_cb))
            .mount(&self.mock_server)
            .await;

        // setMyCommands
        let set_cmds = serde_json::json!({ "ok": true, "result": true });
        Mock::given(method("POST"))
            .and(path_regex("/bot[^/]+/setMyCommands"))
            .respond_with(ResponseTemplate::new(200).set_body_json(set_cmds))
            .mount(&self.mock_server)
            .await;

        // deleteMessage
        let delete_msg = serde_json::json!({ "ok": true, "result": true });
        Mock::given(method("POST"))
            .and(path_regex("/bot[^/]+/deleteMessage"))
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
            .and(path_regex("/bot[^/]+/sendVoice"))
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
            .and(path_regex("/bot[^/]+/sendPhoto"))
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
            .and(path_regex("/bot[^/]+/getMe"))
            .respond_with(ResponseTemplate::new(200).set_body_json(get_me))
            .mount(&self.mock_server)
            .await;

        // Catch-all for any unhandled POST requests - returns a valid "ok" response
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
            .mount(&self.mock_server)
            .await;

        // Also catch GET requests
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fallback))
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
    let text = body["text"].as_str().unwrap_or("");
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
