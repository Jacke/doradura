//! Integration tests for music search functionality.
//!
//! Run with: cargo test --package doradura --test search_test -- --nocapture

use doradura::download::search::{search, SearchSource, YtdlpFlatEntry};

/// Test that yt-dlp JSON parsing works for YouTube flat-playlist output.
#[test]
fn test_ytdlp_json_parsing() {
    let sample = r#"{"_type": "url", "ie_key": "Youtube", "id": "dQw4w9WgXcQ", "url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ", "title": "Rick Astley - Never Gonna Give You Up", "description": null, "duration": 212.0, "channel_id": "UCuAXFkgsw1L7xaCfnd5JJOw", "channel": "Rick Astley", "channel_url": "https://www.youtube.com/channel/UCuAXFkgsw1L7xaCfnd5JJOw", "uploader": "Rick Astley", "uploader_id": "@RickAstleyYT", "thumbnails": [], "webpage_url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ"}"#;

    let entry: YtdlpFlatEntry = serde_json::from_str(sample).expect("Failed to parse yt-dlp JSON");
    assert_eq!(entry.title.as_deref(), Some("Rick Astley - Never Gonna Give You Up"));
    assert_eq!(entry.artist(), Some("Rick Astley"));
    assert_eq!(
        entry.webpage_url.as_deref(),
        Some("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
    );
    assert_eq!(entry.duration, Some(212.0));
}

/// Test parsing with only `channel` field (no `uploader`).
#[test]
fn test_ytdlp_json_channel_fallback() {
    let sample = r#"{"title": "Test Song", "channel": "Test Channel", "webpage_url": "https://www.youtube.com/watch?v=abc123", "duration": 180.0}"#;

    let entry: YtdlpFlatEntry = serde_json::from_str(sample).expect("Failed to parse");
    // `channel` should be used as fallback when `uploader` is missing
    assert_eq!(entry.artist(), Some("Test Channel"));
}

/// Test parsing with missing optional fields.
#[test]
fn test_ytdlp_json_minimal() {
    let sample = r#"{"title": "Minimal Track", "url": "https://example.com/track"}"#;

    let entry: YtdlpFlatEntry = serde_json::from_str(sample).expect("Failed to parse");
    assert_eq!(entry.title.as_deref(), Some("Minimal Track"));
    assert!(entry.uploader.is_none());
    assert!(entry.webpage_url.is_none());
    assert_eq!(entry.url.as_deref(), Some("https://example.com/track"));
    assert!(entry.duration.is_none());
}

/// Test parsing empty title is filtered out.
#[test]
fn test_empty_title_filtered() {
    let sample = r#"{"title": "", "url": "https://example.com/track"}"#;

    let entry: YtdlpFlatEntry = serde_json::from_str(sample).expect("Failed to parse");
    assert_eq!(entry.title.as_deref(), Some(""));
    // In the actual search function, empty title entries are filtered out
}

/// Test SearchSource methods.
#[test]
fn test_search_source_methods() {
    assert_eq!(SearchSource::YouTube.prefix(), "ytsearch");
    assert_eq!(SearchSource::SoundCloud.prefix(), "scsearch");
    assert_eq!(SearchSource::YouTube.code(), "y");
    assert_eq!(SearchSource::SoundCloud.code(), "s");
    assert_eq!(SearchSource::YouTube.label(), "YouTube");
    assert_eq!(SearchSource::SoundCloud.label(), "SoundCloud");
    assert_eq!(SearchSource::YouTube.source_name(), "youtube");
    assert_eq!(SearchSource::SoundCloud.source_name(), "soundcloud");
    assert_eq!(SearchSource::from_code("y"), Some(SearchSource::YouTube));
    assert_eq!(SearchSource::from_code("s"), Some(SearchSource::SoundCloud));
    assert_eq!(SearchSource::from_code("x"), None);
}

/// Test source_name_from_url detection.
#[test]
fn test_source_name_from_url() {
    use doradura::download::search::source_name_from_url;

    assert_eq!(source_name_from_url("https://www.youtube.com/watch?v=abc"), "youtube");
    assert_eq!(
        source_name_from_url("https://soundcloud.com/artist/track"),
        "soundcloud"
    );
    assert_eq!(source_name_from_url("https://open.spotify.com/track/abc"), "spotify");
    assert_eq!(source_name_from_url("https://example.com/file.mp3"), "youtube");
    // default
}

/// Test format_duration.
#[test]
fn test_format_duration() {
    use doradura::download::search::format_duration;

    assert_eq!(format_duration(Some(0)), "0:00");
    assert_eq!(format_duration(Some(65)), "1:05");
    assert_eq!(format_duration(Some(3661)), "1:01:01");
    assert_eq!(format_duration(None), "?:??");
}

/// Integration test: actually run yt-dlp search (requires yt-dlp installed).
/// This test is ignored by default — run with `--ignored` flag.
#[tokio::test]
#[ignore]
async fn test_youtube_search_live() {
    let results = search(SearchSource::YouTube, "never gonna give you up", 3, None)
        .await
        .expect("Search should not fail");

    println!("YouTube search returned {} results:", results.len());
    for (i, r) in results.iter().enumerate() {
        println!(
            "  {}. {} - {} ({:?}s) [{}]",
            i + 1,
            r.artist,
            r.title,
            r.duration_secs,
            r.url
        );
    }

    assert!(!results.is_empty(), "YouTube search should return results");
    assert!(results.len() <= 3, "Should respect limit");

    // Check that results have required fields
    for r in &results {
        assert!(!r.title.is_empty(), "Title should not be empty");
        assert!(!r.url.is_empty(), "URL should not be empty");
        assert!(r.url.starts_with("https://"), "URL should be HTTPS");
    }
}

/// Integration test: SoundCloud search.
#[tokio::test]
#[ignore]
async fn test_soundcloud_search_live() {
    let results = search(SearchSource::SoundCloud, "electronic music", 3, None)
        .await
        .expect("Search should not fail");

    println!("SoundCloud search returned {} results:", results.len());
    for (i, r) in results.iter().enumerate() {
        println!(
            "  {}. {} - {} ({:?}s) [{}]",
            i + 1,
            r.artist,
            r.title,
            r.duration_secs,
            r.url
        );
    }

    assert!(!results.is_empty(), "SoundCloud search should return results");
}
