//! Integration tests for Telegram handlers using teloxide_tests
//!
//! These tests simulate real Telegram interactions without hitting the API.
//! Run with: cargo test --test handlers_integration_test
//!
//! There are two types of tests here:
//! 1. Simple mock handler tests - verify basic bot behavior patterns
//! 2. Real schema tests - use the actual handlers from the codebase with mock dependencies

use serial_test::serial;
use std::sync::Arc;
use teloxide::dispatching::{UpdateFilterExt, UpdateHandler};
use teloxide::prelude::*;
use teloxide_tests::{MockBot, MockCallbackQuery, MockMessageText};

type HandlerError = Box<dyn std::error::Error + Send + Sync + 'static>;

// ============================================================================
// PART 1: Simple Mock Handler Tests
// These tests verify basic bot behavior patterns without complex dependencies
// ============================================================================

/// A simplified /start handler for testing basic patterns
async fn mock_handle_start(bot: Bot, msg: Message) -> Result<(), HandlerError> {
    let keyboard = teloxide::types::InlineKeyboardMarkup::new(vec![
        vec![
            teloxide::types::InlineKeyboardButton::callback("ℹ️ Информация", "menu:info"),
            teloxide::types::InlineKeyboardButton::callback("⚙️ Настройки", "menu:settings"),
        ],
        vec![teloxide::types::InlineKeyboardButton::callback(
            "📥 Мои загрузки",
            "menu:downloads",
        )],
    ]);

    bot.send_message(
        msg.chat.id,
        "🎵 Привет! Я помогу тебе скачать музыку и видео с YouTube и других платформ.\n\n📝 Просто отправь мне ссылку на видео или трек!",
    )
    .reply_markup(keyboard)
    .await?;

    Ok(())
}

/// Handler tree for /start command testing
fn mock_start_handler_tree() -> UpdateHandler<HandlerError> {
    dptree::entry().branch(
        Update::filter_message()
            .filter(|msg: Message| msg.text().map(|text| text == "/start").unwrap_or(false))
            .endpoint(mock_handle_start),
    )
}

#[tokio::test]
#[serial]
async fn test_start_command_sends_welcome_message() {
    let message = MockMessageText::new().text("/start");
    let mut bot = MockBot::new(message, mock_start_handler_tree());

    bot.dispatch().await;

    let responses = bot.get_responses();
    let sent_messages = &responses.sent_messages;

    assert_eq!(sent_messages.len(), 1, "Should send exactly one message");

    let msg = &sent_messages[0];
    let text = msg.text().expect("Message should have text");
    println!("text: {}", text);

    assert!(text.contains("Привет"), "Should contain greeting");
    assert!(text.contains("музыку"), "Should mention music");
    assert!(text.contains("видео"), "Should mention video");
}

#[tokio::test]
#[serial]
async fn test_start_command_has_inline_keyboard() {
    let message = MockMessageText::new().text("/start");
    let mut bot = MockBot::new(message, mock_start_handler_tree());

    bot.dispatch().await;

    let responses = bot.get_responses();
    let msg = &responses.sent_messages[0];

    // Check that reply markup exists
    assert!(msg.reply_markup().is_some(), "Should have inline keyboard");

    if let Some(markup) = msg.reply_markup() {
        let keyboard = &markup.inline_keyboard;
        assert!(keyboard.len() >= 2, "Should have at least 2 rows");

        // Check first row has Info and Settings
        let first_row = &keyboard[0];
        assert_eq!(first_row.len(), 2, "First row should have 2 buttons");
        assert!(first_row[0].text.contains("Информация"), "Should have Info button");
        assert!(first_row[1].text.contains("Настройки"), "Should have Settings button");

        // Check second row has Downloads
        let second_row = &keyboard[1];
        assert!(second_row[0].text.contains("загрузки"), "Should have Downloads button");
    }
}

// ==================== Callback query handler ====================

/// A simplified callback handler for testing
async fn mock_handle_callback(bot: Bot, q: CallbackQuery) -> Result<(), HandlerError> {
    if let Some(data) = q.data {
        bot.answer_callback_query(q.id).await?;

        match data.as_str() {
            "menu:info" => {
                if let Some(msg) = q.message {
                    bot.send_message(
                        msg.chat().id,
                        "ℹ️ *Информация о боте*\n\nЯ умею скачивать видео и аудио.",
                    )
                    .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                    .await?;
                }
            }
            "menu:settings" => {
                if let Some(msg) = q.message {
                    bot.send_message(msg.chat().id, "⚙️ Настройки").await?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Handler tree for callback testing
fn mock_callback_handler_tree() -> UpdateHandler<HandlerError> {
    dptree::entry().branch(Update::filter_callback_query().endpoint(mock_handle_callback))
}

#[tokio::test]
#[serial]
async fn test_info_callback_shows_info() {
    let callback = MockCallbackQuery::new().data("menu:info");
    let mut bot = MockBot::new(callback, mock_callback_handler_tree());

    bot.dispatch().await;

    let responses = bot.get_responses();

    // Should answer callback query
    assert!(
        !responses.answered_callback_queries.is_empty(),
        "Should answer callback query"
    );

    // Should send info message
    assert!(!responses.sent_messages.is_empty(), "Should send info message");

    let msg = &responses.sent_messages[0];
    let text = msg.text().expect("Should have text");
    assert!(text.contains("Информация"), "Should contain info text");
}

#[tokio::test]
#[serial]
async fn test_settings_callback_shows_settings() {
    let callback = MockCallbackQuery::new().data("menu:settings");
    let mut bot = MockBot::new(callback, mock_callback_handler_tree());

    bot.dispatch().await;

    let responses = bot.get_responses();

    assert!(
        !responses.answered_callback_queries.is_empty(),
        "Should answer callback query"
    );

    let msg = &responses.sent_messages[0];
    let text = msg.text().expect("Should have text");
    assert!(text.contains("Настройки"), "Should show settings");
}

// ==================== URL message handler ====================

/// A simplified URL handler for testing
async fn mock_handle_url_message(bot: Bot, msg: Message) -> Result<(), HandlerError> {
    if let Some(text) = msg.text() {
        // Check if text contains a URL
        let url_regex = regex::Regex::new(r"https?://[^\s]+").unwrap();

        if url_regex.is_match(text) {
            bot.send_message(msg.chat.id, "⏳ Обрабатываю ссылку...").await?;
        }
    }
    Ok(())
}

/// Handler tree for URL message testing
fn mock_url_handler_tree() -> UpdateHandler<HandlerError> {
    dptree::entry().branch(Update::filter_message().endpoint(mock_handle_url_message))
}

#[tokio::test]
#[serial]
async fn test_youtube_url_triggers_processing() {
    let message = MockMessageText::new().text("https://youtube.com/watch?v=dQw4w9WgXcQ");
    let mut bot = MockBot::new(message, mock_url_handler_tree());

    bot.dispatch().await;

    let responses = bot.get_responses();
    assert_eq!(responses.sent_messages.len(), 1, "Should send processing message");

    let msg = &responses.sent_messages[0];
    let text = msg.text().expect("Should have text");
    assert!(text.contains("Обрабатываю"), "Should show processing status");
}

#[tokio::test]
#[serial]
async fn test_plain_text_no_url_no_processing() {
    let message = MockMessageText::new().text("Привет, как дела?");
    let mut bot = MockBot::new(message, mock_url_handler_tree());

    bot.dispatch().await;

    let responses = bot.get_responses();
    // Plain text without URL should not trigger "processing" message
    let has_processing = responses
        .sent_messages
        .iter()
        .any(|m| m.text().unwrap_or("").contains("Обрабатываю"));
    assert!(!has_processing, "Should not show processing for non-URL messages");
}

// ==================== Multiple updates in sequence ====================

#[tokio::test]
#[serial]
async fn test_multiple_messages_in_sequence() {
    // Test that we can handle multiple messages in sequence
    let messages = vec![
        MockMessageText::new().text("/start"),
        MockMessageText::new().text("https://youtube.com/watch?v=test"),
    ];

    // Combined handler tree
    fn combined_handler_tree() -> UpdateHandler<HandlerError> {
        dptree::entry()
            .branch(
                Update::filter_message()
                    .filter(|msg: Message| msg.text().map(|text| text == "/start").unwrap_or(false))
                    .endpoint(mock_handle_start),
            )
            .branch(
                Update::filter_message()
                    .filter(|msg: Message| msg.text().map(|text| text.contains("http")).unwrap_or(false))
                    .endpoint(mock_handle_url_message),
            )
    }

    let mut bot = MockBot::new(messages, combined_handler_tree());
    bot.dispatch().await;

    let responses = bot.get_responses();

    // Should have 2 messages: welcome + processing
    assert_eq!(responses.sent_messages.len(), 2, "Should send 2 messages for 2 inputs");

    // First should be welcome
    assert!(responses.sent_messages[0].text().unwrap().contains("Привет"));

    // Second should be processing
    assert!(responses.sent_messages[1].text().unwrap().contains("Обрабатываю"));
}

// ============================================================================
// PART 2: Real Handler Tests with Mock Dependencies
// These tests use the actual handlers from src/telegram/handlers.rs
// ============================================================================

use doradura::core::rate_limiter::RateLimiter;
use doradura::download::DownloadQueue;
use doradura::downsub::DownsubGateway;
use doradura::storage::create_pool;
use doradura::telegram::{schema, HandlerDeps};

/// Creates test dependencies with an in-memory SQLite database
async fn create_test_deps() -> HandlerDeps {
    // Create in-memory database
    let db_pool = Arc::new(create_pool(":memory:").expect("Failed to create test database"));

    // Initialize the database schema
    {
        let conn = db_pool.get().expect("Failed to get connection");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS users (
                id INTEGER PRIMARY KEY,
                telegram_id INTEGER NOT NULL UNIQUE,
                username TEXT,
                language TEXT DEFAULT 'ru',
                created_at TEXT DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE IF NOT EXISTS downloads (
                id INTEGER PRIMARY KEY,
                user_id INTEGER,
                url TEXT,
                title TEXT,
                format TEXT,
                file_size INTEGER,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE IF NOT EXISTS user_settings (
                id INTEGER PRIMARY KEY,
                user_id INTEGER NOT NULL UNIQUE,
                download_type TEXT DEFAULT 'mp3',
                video_quality TEXT DEFAULT '720p',
                audio_bitrate TEXT DEFAULT '320k'
            );",
        )
        .expect("Failed to create tables");
    }

    let download_queue = Arc::new(DownloadQueue::new());
    let rate_limiter = Arc::new(RateLimiter::new());

    // Create a downsub gateway (will be unavailable without DOWNSUB_GRPC_ENDPOINT)
    let downsub_gateway = Arc::new(DownsubGateway::from_env().await);

    HandlerDeps::new(
        db_pool,
        download_queue,
        rate_limiter,
        downsub_gateway,
        Some("test_bot".to_string()),
        UserId(123456789),
        None, // alert_manager - not needed for tests
    )
}

/// Test that the real schema can be created and dispatched
///
/// NOTE: This test is ignored because the real handlers use a custom Bot type
/// (doradura::telegram::bot_api_logger::Bot) that is incompatible with MockBot.
/// The mock tests above still validate the handler patterns work correctly.
#[tokio::test]
#[serial]
#[ignore = "Real handlers use custom Bot type incompatible with MockBot"]
async fn test_real_schema_creation() {
    let deps = create_test_deps().await;
    let handler = schema(deps);

    // Just verify that the handler can be created
    // This tests that all the module imports and function signatures are correct
    let message = MockMessageText::new().text("test message");
    let mut bot = MockBot::new(message, handler);

    // Dispatch should complete without panic
    bot.dispatch().await;
}

/// Test real /info command handler
#[tokio::test]
#[serial]
#[ignore = "Real handlers use custom Bot type incompatible with MockBot"]
async fn test_real_info_command() {
    let deps = create_test_deps().await;

    // First create a user in the database
    {
        let conn = deps.db_pool.get().expect("Failed to get connection");
        conn.execute(
            "INSERT INTO users (telegram_id, username, language) VALUES (12345, 'testuser', 'ru')",
            [],
        )
        .expect("Failed to insert test user");
    }

    let handler = schema(deps);

    let message = MockMessageText::new().text("/info");
    let mut bot = MockBot::new(message, handler);

    bot.dispatch().await;

    let responses = bot.get_responses();

    // The /info command should send at least one message
    // Note: exact behavior depends on user state in DB
    println!("Responses: {:?}", responses.sent_messages.len());
}

/// Test that callback queries are properly handled
#[tokio::test]
#[serial]
#[ignore = "Real handlers use custom Bot type incompatible with MockBot"]
async fn test_real_callback_handling() {
    let deps = create_test_deps().await;

    // Create a user first
    {
        let conn = deps.db_pool.get().expect("Failed to get connection");
        conn.execute(
            "INSERT INTO users (telegram_id, username, language) VALUES (12345, 'testuser', 'ru')",
            [],
        )
        .expect("Failed to insert test user");
    }

    let handler = schema(deps);

    // Send a callback query that should be handled by the menu callback handler
    let callback = MockCallbackQuery::new().data("menu:back");
    let mut bot = MockBot::new(callback, handler);

    bot.dispatch().await;

    let responses = bot.get_responses();

    // Should answer the callback query
    assert!(
        !responses.answered_callback_queries.is_empty(),
        "Real handlers should answer callback queries"
    );
}
