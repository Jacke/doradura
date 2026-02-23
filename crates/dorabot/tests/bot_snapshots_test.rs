//! Snapshot-based bot testing
//!
//! These tests use recorded Telegram API interactions to verify bot behavior
//! without making actual API calls.
//!
//! Run with: cargo test --test bot_snapshots_test

mod common;

use common::{TelegramMock, TelegramSnapshot};

#[tokio::test]
async fn test_start_command_from_snapshot() {
    // Load the snapshot
    let mock = TelegramMock::from_snapshot("start_command")
        .await
        .expect("Failed to load snapshot");

    // Create a bot that uses the mock server
    let _bot = mock.create_bot().expect("Failed to create bot");

    // In a real test, you would:
    // 1. Call your bot handler functions
    // 2. The handler would make API calls through the bot
    // 3. The mock would respond based on the snapshot
    // 4. Verify the responses

    // For now, just verify the snapshot structure is correct
    let snapshot = mock.snapshot();
    assert_eq!(snapshot.name, "start_command");
    assert_eq!(snapshot.interactions.len(), 1);

    // Note: verify() would check that all expected API calls were made
    // In a real test where you actually call bot methods, you would:
    // mock.verify().await.expect("Verification failed");
}

#[tokio::test]
async fn test_snapshot_contains_expected_data() {
    let snapshot = TelegramSnapshot::load_by_name("start_command").expect("Failed to load snapshot");

    assert_eq!(snapshot.name, "start_command");
    assert_eq!(snapshot.interactions.len(), 1);

    let (call, response) = &snapshot.interactions[0];
    assert_eq!(call.method, "POST");
    assert_eq!(call.path, "/sendMessage");
    assert_eq!(response.status, 200);

    // Verify response structure
    assert!(response.body["ok"].as_bool().unwrap_or(false));
    assert!(response.body["result"]["message_id"].is_number());
}

#[test]
fn test_snapshot_loading() {
    let result = TelegramSnapshot::load_by_name("start_command");
    assert!(result.is_ok(), "Should be able to load start_command snapshot");

    let snapshot = result.unwrap();
    assert!(!snapshot.interactions.is_empty(), "Snapshot should have interactions");
}

/// Example of how to use snapshots for testing download flow
#[tokio::test]
#[ignore] // Remove this when you create the snapshot
async fn test_download_youtube_video_snapshot() {
    // This test would use a snapshot of a complete download flow:
    // 1. User sends YouTube URL
    // 2. Bot sends "Processing..." message
    // 3. Bot sends preview with quality options
    // 4. User selects quality
    // 5. Bot downloads and sends file

    let mock = TelegramMock::from_snapshot("youtube_download_flow")
        .await
        .expect("Failed to load snapshot");

    let _bot = mock.create_bot().expect("Failed to create bot");

    // Test would go here...

    mock.verify().await.expect("Verification failed");
}
