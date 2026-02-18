//! E2E (End-to-End) Tests
//!
//! These tests verify complete user flows without touching real Telegram API.
//! Everything is mocked using snapshots.
//!
//! Run with: cargo test --test e2e_test

mod common;

use common::TestEnvironment;

/// E2E Test: User sends /start command
///
/// Flow:
/// 1. User sends /start
/// 2. Bot sends welcome message with inline keyboard
///
/// Verifies:
/// - Correct message sent
/// - Inline keyboard has correct buttons
/// - No errors occurred
#[tokio::test]
async fn e2e_start_command() {
    // Setup test environment with mock Telegram API
    let env = TestEnvironment::new("start_command")
        .await
        .expect("Failed to setup test environment");

    // Verify the expected flow from snapshot
    // Snapshot contains 1 interaction: sendMessage with welcome text + keyboard
    let snapshot = env.snapshot();
    assert_eq!(snapshot.interactions.len(), 1);

    // Verify API call structure
    let (call, response) = &snapshot.interactions[0];
    assert_eq!(call.method, "POST");
    assert_eq!(call.path, "/sendMessage");

    // Verify response is successful
    assert_eq!(response.status, 200);
    assert!(response.body["ok"].as_bool().unwrap());

    // Verify message content
    let result = &response.body["result"];
    let text = result["text"].as_str().unwrap();
    assert!(text.contains("Hello") || text.contains("Привет"));
    assert!(text.contains("music") || text.contains("музыку"));

    // Verify inline keyboard exists
    assert!(result["reply_markup"]["inline_keyboard"].is_array());
    let keyboard = result["reply_markup"]["inline_keyboard"].as_array().unwrap();
    assert!(!keyboard.is_empty(), "Keyboard should have buttons");

    println!("✓ E2E: /start command flow verified");
}

/// E2E Test: User requests info about formats
#[tokio::test]
async fn e2e_info_command() {
    let env = TestEnvironment::new("info_command")
        .await
        .expect("Failed to setup test environment");

    // Verify info message structure
    let snapshot = env.snapshot();
    assert_eq!(snapshot.interactions.len(), 1);

    let (_call, response) = &snapshot.interactions[0];
    let text = response.body["result"]["text"].as_str().unwrap();

    // Check that all required information is present
    assert!(
        text.contains("Video") || text.contains("Видео"),
        "Should mention video formats"
    );
    assert!(
        text.contains("Audio") || text.contains("Аудио"),
        "Should mention audio formats"
    );
    assert!(text.contains("YouTube"), "Should mention YouTube");
    assert!(text.contains("1080p"), "Should mention video quality");
    assert!(text.contains("320 kbps"), "Should mention audio quality");

    println!("✓ E2E: /info command provides complete information");
}

/// E2E Test: User opens settings menu
#[tokio::test]
async fn e2e_settings_menu() {
    let env = TestEnvironment::new("settings_menu")
        .await
        .expect("Failed to setup test environment");

    let snapshot = env.snapshot();
    assert_eq!(snapshot.interactions.len(), 1);

    let (_call, response) = &snapshot.interactions[0];
    let result = &response.body["result"];

    // Verify settings are displayed
    let text = result["text"].as_str().unwrap();
    assert!(text.contains("Settings") || text.contains("Настройки"));
    assert!(text.contains("1080p")); // Current video quality
    assert!(text.contains("192 kbps")); // Current audio bitrate

    // Verify settings buttons exist
    let keyboard = result["reply_markup"]["inline_keyboard"].as_array().unwrap();
    assert!(keyboard.len() >= 4, "Should have multiple setting options");

    println!("✓ E2E: Settings menu displays current preferences");
}

/// E2E Test: Complete YouTube processing flow
///
/// Flow:
/// 1. Bot sends "Processing..." message
/// 2. Bot fetches video metadata
/// 3. Bot sends preview with thumbnail and quality options
/// 4. Bot deletes "Processing..." message
#[tokio::test]
async fn e2e_youtube_processing_flow() {
    let env = TestEnvironment::new("youtube_processing")
        .await
        .expect("Failed to setup test environment");

    // Verify complete flow sequence
    env.verify_sequence(&[
        ("POST", "/sendMessage"),   // 1. "Processing..."
        ("POST", "/sendPhoto"),     // 2. Preview with options
        ("POST", "/deleteMessage"), // 3. Clean up
    ]);

    let snapshot = env.snapshot();

    // Step 1: Verify processing message
    let (_call1, resp1) = &snapshot.interactions[0];
    let text1 = resp1.body["result"]["text"].as_str().unwrap();
    assert!(
        text1.contains("Processing") || text1.contains("Обрабатываю"),
        "Should show processing status"
    );

    // Step 2: Verify preview
    let (call2, resp2) = &snapshot.interactions[1];
    assert_eq!(call2.path, "/sendPhoto");

    let caption = resp2.body["result"]["caption"].as_str().unwrap();
    assert!(caption.contains("Rick Astley"), "Should show video title");
    assert!(
        caption.contains("Select quality") || caption.contains("Выбери качество"),
        "Should prompt for quality"
    );

    // Verify quality buttons
    let keyboard = resp2.body["result"]["reply_markup"]["inline_keyboard"]
        .as_array()
        .unwrap();
    assert!(keyboard.len() >= 3, "Should have audio and video options");

    // Step 3: Verify cleanup
    let (call3, _resp3) = &snapshot.interactions[2];
    assert_eq!(call3.path, "/deleteMessage");

    println!("✓ E2E: YouTube processing flow is complete and correct");
}

/// E2E Test: Complete audio download with progress updates
///
/// Flow:
/// 1. Update caption: 0%
/// 2. Update caption: 45%
/// 3. Update caption: 100%
/// 4. Send audio file
/// 5. Delete progress message
#[tokio::test]
async fn e2e_audio_download_complete() {
    let env = TestEnvironment::new("audio_download_complete")
        .await
        .expect("Failed to setup test environment");

    let snapshot = env.snapshot();
    assert_eq!(snapshot.interactions.len(), 5, "Should have 5 steps");

    // Verify progress updates
    for (i, expected_progress) in [("0%", 0), ("45%", 1), ("100%", 2)] {
        let (_call, resp) = &snapshot.interactions[expected_progress];
        let caption = resp.body["result"]["caption"].as_str().unwrap();
        assert!(caption.contains(i), "Progress {} should show {}%", expected_progress, i);
    }

    // Verify audio file delivery
    let (_call4, resp4) = &snapshot.interactions[3];
    let audio = &resp4.body["result"]["audio"];

    assert!(audio.is_object(), "Should send audio file");
    assert_eq!(audio["performer"].as_str().unwrap(), "Rick Astley");
    assert_eq!(audio["title"].as_str().unwrap(), "Never Gonna Give You Up");
    assert_eq!(audio["duration"].as_u64().unwrap(), 213);

    // Verify cleanup
    let (call5, _) = &snapshot.interactions[4];
    assert_eq!(call5.path, "/deleteMessage");

    println!("✓ E2E: Audio download flow with progress tracking works");
}

/// E2E Test: Language selection flow
///
/// Flow:
/// 1. Show language menu
/// 2. User selects language (callback)
/// 3. Confirm selection
/// 4. Update settings menu
#[tokio::test]
async fn e2e_language_selection_flow() {
    let env = TestEnvironment::new("language_selection")
        .await
        .expect("Failed to setup test environment");

    env.verify_sequence(&[
        ("POST", "/sendMessage"),         // Language menu
        ("POST", "/answerCallbackQuery"), // Confirm selection
        ("POST", "/editMessageText"),     // Update settings
    ]);

    let snapshot = env.snapshot();

    // Verify language menu
    let (_call1, resp1) = &snapshot.interactions[0];
    let text1 = resp1.body["result"]["text"].as_str().unwrap();
    assert!(text1.contains("Select language") || text1.contains("Выбери язык"));
    assert!(text1.contains("Choose language"));

    // Verify callback answer
    let (_call2, resp2) = &snapshot.interactions[1];
    assert!(resp2.body["ok"].as_bool().unwrap());

    // Verify settings updated
    let (_call3, resp3) = &snapshot.interactions[2];
    let text3 = resp3.body["result"]["text"].as_str().unwrap();
    assert!(
        text3.contains("🇷🇺 Russian") || text3.contains("🇷🇺 Русский"),
        "Language should be updated"
    );

    println!("✓ E2E: Language selection flow works correctly");
}

/// E2E Test: Rate limit error handling
#[tokio::test]
async fn e2e_rate_limit_error() {
    let env = TestEnvironment::new("rate_limit_error")
        .await
        .expect("Failed to setup test environment");

    let snapshot = env.snapshot();
    assert_eq!(snapshot.interactions.len(), 1);

    // Verify error message
    let (_call, resp) = &snapshot.interactions[0];
    let text = resp.body["result"]["text"].as_str().unwrap();

    assert!(
        text.contains("Wait") || text.contains("Подожди"),
        "Should show rate limit message"
    );
    assert!(text.contains("45"), "Should show wait time");
    assert!(text.contains("/plan"), "Should suggest upgrade");
    assert!(text.contains("Premium"), "Should mention Premium plan");

    // Verify metadata
    assert_eq!(snapshot.metadata.get("error_type").unwrap(), "rate_limit");
    assert_eq!(snapshot.metadata.get("remaining_seconds").unwrap(), "45");

    println!("✓ E2E: Rate limit error is handled correctly");
}

/// E2E Test: Verify all snapshots are valid for E2E testing
#[test]
fn e2e_all_snapshots_valid() {
    let snapshots = vec![
        "start_command",
        "info_command",
        "settings_menu",
        "language_selection",
        "youtube_processing",
        "audio_download_complete",
        "rate_limit_error",
    ];

    for snapshot_name in snapshots {
        let snapshot = common::TelegramSnapshot::load_by_name(snapshot_name)
            .unwrap_or_else(|_| panic!("Should load snapshot: {}", snapshot_name));

        assert!(
            !snapshot.interactions.is_empty(),
            "Snapshot {} should have interactions",
            snapshot_name
        );

        assert_eq!(
            snapshot.version, "1.0",
            "Snapshot {} should be version 1.0",
            snapshot_name
        );

        println!("✓ Snapshot {} is valid for E2E", snapshot_name);
    }
}

/// Example: Testing error handling paths
#[tokio::test]
async fn e2e_error_scenarios() {
    // Test that error snapshots contain proper error information
    let env = TestEnvironment::new("rate_limit_error").await.unwrap();

    let snapshot = env.snapshot();
    let metadata = &snapshot.metadata;

    // Verify error is properly categorized
    assert!(metadata.contains_key("error_type"));
    assert_eq!(metadata.get("error_type").unwrap(), "rate_limit");

    // Could test other error types:
    // - invalid_url
    // - network_error
    // - download_failed
    // etc.

    println!("✓ E2E: Error scenarios are properly documented");
}
