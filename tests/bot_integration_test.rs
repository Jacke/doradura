//! Integration tests - Documentation and examples
//!
//! This file demonstrates how to test bot handlers with snapshots.
//! Full integration requires creating Message objects, which is complex in teloxide.
//!
//! Recommended approach: Test business logic separately from Telegram types.

mod common;

/// Example: Verify snapshot structure for a command
#[test]
fn test_start_command_snapshot_structure() {
    let snapshot = common::TelegramSnapshot::load_by_name("start_command").expect("Failed to load snapshot");

    // Verify metadata
    assert_eq!(snapshot.metadata.get("command"), Some(&"/start".to_string()));
    assert_eq!(snapshot.interactions.len(), 1);

    // Verify API call structure
    let (call, response) = &snapshot.interactions[0];
    assert_eq!(call.method, "POST");
    assert_eq!(call.path, "/sendMessage");
    assert_eq!(response.status, 200);
}

/// Example: Check that YouTube processing has the right flow
#[test]
fn test_youtube_processing_flow_structure() {
    let snapshot = common::TelegramSnapshot::load_by_name("youtube_processing").expect("Failed to load snapshot");

    assert_eq!(snapshot.interactions.len(), 3, "Should have 3 API calls");

    // Check the sequence
    let call_paths: Vec<&str> = snapshot
        .interactions
        .iter()
        .map(|(call, _)| call.path.as_str())
        .collect();

    assert_eq!(call_paths, vec!["/sendMessage", "/sendPhoto", "/deleteMessage"]);
}
