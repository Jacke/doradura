//! Tests for bot commands using snapshots

mod common;

use common::{TelegramMock, TelegramSnapshot};

#[tokio::test]
async fn test_info_command_snapshot() {
    let mock = TelegramMock::from_snapshot("info_command")
        .await
        .expect("Failed to load info_command snapshot");

    let _bot = mock.create_bot().expect("Failed to create bot");

    let snapshot = mock.snapshot();
    assert_eq!(snapshot.name, "info_command");
    assert_eq!(snapshot.interactions.len(), 1);

    // Verify the response contains expected information
    let (_call, response) = &snapshot.interactions[0];
    assert_eq!(response.status, 200);
    assert!(response.body["ok"].as_bool().unwrap());

    let result = &response.body["result"];
    let text = result["text"].as_str().unwrap();

    // Check that info message contains key information
    assert!(text.contains("Video") || text.contains("Видео"));
    assert!(text.contains("Audio") || text.contains("Аудио"));
    assert!(text.contains("YouTube"));
    assert!(text.contains("320 kbps"));
    assert!(text.contains("1080p"));
}

#[tokio::test]
async fn test_settings_menu_snapshot() {
    let mock = TelegramMock::from_snapshot("settings_menu")
        .await
        .expect("Failed to load settings_menu snapshot");

    let _bot = mock.create_bot().expect("Failed to create bot");

    let snapshot = mock.snapshot();
    assert_eq!(snapshot.name, "settings_menu");
    assert_eq!(snapshot.interactions.len(), 1);

    let (_call, response) = &snapshot.interactions[0];
    let result = &response.body["result"];

    // Verify inline keyboard is present
    assert!(result["reply_markup"]["inline_keyboard"].is_array());

    let keyboard = result["reply_markup"]["inline_keyboard"].as_array().unwrap();
    assert!(keyboard.len() >= 4, "Should have at least 4 rows of buttons");

    // Verify text contains current settings
    let text = result["text"].as_str().unwrap();
    assert!(text.contains("Settings") || text.contains("Настройки"));
    assert!(text.contains("1080p"));
    assert!(text.contains("192 kbps"));
}

#[tokio::test]
async fn test_rate_limit_error_snapshot() {
    let snapshot =
        TelegramSnapshot::load_by_name("rate_limit_error").expect("Failed to load rate_limit_error snapshot");

    assert_eq!(snapshot.name, "rate_limit_error");

    // Check metadata
    assert_eq!(snapshot.metadata.get("error_type").unwrap(), "rate_limit");
    assert_eq!(snapshot.metadata.get("remaining_seconds").unwrap(), "45");

    // Verify error message
    let (_call, response) = &snapshot.interactions[0];
    let text = response.body["result"]["text"].as_str().unwrap();

    assert!(text.contains("Wait") || text.contains("Подожди"));
    assert!(text.contains("45"));
    assert!(text.contains("/plan"));
    assert!(text.contains("Premium"));
}

#[tokio::test]
async fn test_language_selection_flow() {
    let snapshot =
        TelegramSnapshot::load_by_name("language_selection").expect("Failed to load language_selection snapshot");

    assert_eq!(snapshot.interactions.len(), 3);

    // 1. Show language menu
    let (_call1, response1) = &snapshot.interactions[0];
    let text1 = response1.body["result"]["text"].as_str().unwrap();
    assert!(text1.contains("Select language") || text1.contains("Выбери язык"));
    assert!(text1.contains("Choose language"));

    // 2. Answer callback query
    let (_call2, response2) = &snapshot.interactions[1];
    assert_eq!(response2.body["ok"], true);

    // 3. Update settings with new language
    let (_call3, response3) = &snapshot.interactions[2];
    let text3 = response3.body["result"]["text"].as_str().unwrap();
    assert!(text3.contains("🇷🇺 Russian") || text3.contains("🇷🇺 Русский"));
}

#[tokio::test]
async fn test_youtube_processing_flow() {
    let snapshot =
        TelegramSnapshot::load_by_name("youtube_processing").expect("Failed to load youtube_processing snapshot");

    assert_eq!(snapshot.interactions.len(), 3);
    assert_eq!(snapshot.metadata.get("flow").unwrap(), "url_processing");

    // 1. Processing message
    let (call1, response1) = &snapshot.interactions[0];
    assert_eq!(call1.path, "/sendMessage");
    assert!(
        response1.body["result"]["text"]
            .as_str()
            .unwrap()
            .contains("Processing")
            || response1.body["result"]["text"]
                .as_str()
                .unwrap()
                .contains("Обрабатываю")
    );

    // 2. Preview with quality options
    let (call2, response2) = &snapshot.interactions[1];
    assert_eq!(call2.path, "/sendPhoto");

    let caption = response2.body["result"]["caption"].as_str().unwrap();
    assert!(caption.contains("Rick Astley"));
    assert!(caption.contains("Select quality") || caption.contains("Выбери качество"));

    // Verify inline keyboard has download options
    let keyboard = response2.body["result"]["reply_markup"]["inline_keyboard"]
        .as_array()
        .unwrap();
    assert!(keyboard.len() >= 3, "Should have audio, video and cancel options");

    // 3. Delete processing message
    let (call3, _response3) = &snapshot.interactions[2];
    assert_eq!(call3.path, "/deleteMessage");
}

#[tokio::test]
async fn test_audio_download_complete_flow() {
    let snapshot = TelegramSnapshot::load_by_name("audio_download_complete")
        .expect("Failed to load audio_download_complete snapshot");

    assert_eq!(snapshot.interactions.len(), 5);
    assert_eq!(snapshot.metadata.get("flow").unwrap(), "audio_download_complete");

    // 1. Start download (0% progress)
    let (_call1, response1) = &snapshot.interactions[0];
    let caption1 = response1.body["result"]["caption"].as_str().unwrap();
    assert!(caption1.contains("0%"));

    // 2. Mid-download progress (45%)
    let (_call2, response2) = &snapshot.interactions[1];
    let caption2 = response2.body["result"]["caption"].as_str().unwrap();
    assert!(caption2.contains("45%"));

    // 3. Download complete (100%)
    let (_call3, response3) = &snapshot.interactions[2];
    let caption3 = response3.body["result"]["caption"].as_str().unwrap();
    assert!(caption3.contains("100%"));

    // 4. Send audio file
    let (call4, response4) = &snapshot.interactions[3];
    assert_eq!(call4.path, "/sendAudio");

    let audio = &response4.body["result"]["audio"];
    assert!(audio.is_object());
    assert_eq!(audio["performer"].as_str().unwrap(), "Rick Astley");
    assert_eq!(audio["title"].as_str().unwrap(), "Never Gonna Give You Up");
    assert_eq!(audio["duration"].as_u64().unwrap(), 213);

    // Verify file size
    let file_size = audio["file_size"].as_u64().unwrap();
    assert!(file_size > 0, "File size should be greater than 0");
    assert_eq!(
        snapshot.metadata.get("file_size_bytes").unwrap(),
        &file_size.to_string()
    );

    // 5. Delete progress message
    let (call5, _response5) = &snapshot.interactions[4];
    assert_eq!(call5.path, "/deleteMessage");
}

#[test]
fn test_all_snapshots_are_valid() {
    // Verify all snapshot files can be loaded
    let snapshots = vec![
        "start_command",
        "info_command",
        "settings_menu",
        "rate_limit_error",
        "language_selection",
        "youtube_processing",
        "audio_download_complete",
    ];

    for snapshot_name in snapshots {
        let result = TelegramSnapshot::load_by_name(snapshot_name);
        assert!(
            result.is_ok(),
            "Failed to load snapshot '{}': {:?}",
            snapshot_name,
            result.err()
        );

        let snapshot = result.unwrap();
        assert_eq!(snapshot.name, snapshot_name);
        assert!(
            !snapshot.interactions.is_empty(),
            "Snapshot '{}' has no interactions",
            snapshot_name
        );
        assert_eq!(snapshot.version, "1.0");
    }
}
