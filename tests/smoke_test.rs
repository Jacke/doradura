//! Smoke tests for CI and manual verification.
//!
//! These tests verify the bot's core functionality by performing real downloads
//! from YouTube using the "Me at the zoo" video (first YouTube video, ~19 seconds).
//!
//! # Running Tests
//!
//! ```bash
//! # Run all smoke tests
//! cargo test --test smoke_test -- --nocapture
//!
//! # Run only audio test
//! cargo test --test smoke_test smoke_test_audio_only -- --ignored --nocapture
//!
//! # Run only video test
//! cargo test --test smoke_test smoke_test_video_only -- --ignored --nocapture
//! ```
//!
//! # Environment Variables
//!
//! - `YTDL_COOKIES_FILE` - Path to cookies file for YouTube authentication
//! - `YTDL_COOKIES_BROWSER` - Browser to extract cookies from (chrome, firefox, etc.)
//! - `WARP_PROXY` - WARP proxy URL (optional)
//! - `PROXY_LIST` - Residential proxy list (optional)

use doradura::smoke_tests::{
    is_ffmpeg_available, is_ffprobe_available, is_ytdlp_available, run_all_smoke_tests, test_audio_download,
    test_cookies_validation, test_ffmpeg_toolchain, test_metadata_extraction, test_video_download, SmokeTestConfig,
    SmokeTestStatus, DEFAULT_TEST_URL,
};
use std::time::Duration;

/// Initialize logging for tests
fn init_logging() {
    let _ = pretty_env_logger::try_init();
}

/// Helper to get proxy chain for tests
fn get_test_proxy_chain() -> Vec<Option<doradura::download::metadata::ProxyConfig>> {
    doradura::smoke_tests::runner::get_smoke_test_proxy_chain(true)
}

/// Full smoke test suite - runs all 5 tests.
///
/// This is the main entry point for CI. It runs all smoke tests and
/// verifies that the bot can download audio and video from YouTube.
#[tokio::test]
async fn smoke_test_full_suite() {
    init_logging();

    let config = SmokeTestConfig::for_ci();
    let report = run_all_smoke_tests(&config).await;

    println!("\n{}\n", report.format_log());

    // Allow skipped tests, but not failures
    let has_failures = report
        .results
        .iter()
        .any(|r| matches!(r.status, SmokeTestStatus::Failed | SmokeTestStatus::Timeout));

    if has_failures {
        for result in &report.results {
            if matches!(result.status, SmokeTestStatus::Failed | SmokeTestStatus::Timeout) {
                eprintln!("FAILED: {} - {:?}", result.test_name, result.error_message);
            }
        }
        panic!(
            "Smoke tests failed: {}/{} passed",
            report.passed_count,
            report.results.len()
        );
    }

    println!(
        "All smoke tests passed: {}/{} OK",
        report.passed_count,
        report.results.len()
    );
}

/// Test only the FFmpeg toolchain.
///
/// Verifies that ffmpeg, ffprobe, and yt-dlp are available.
#[tokio::test]
async fn smoke_test_toolchain() {
    init_logging();

    let result = test_ffmpeg_toolchain().await;

    println!("{}", result.format_log());

    assert!(
        result.status == SmokeTestStatus::Passed,
        "FFmpeg toolchain test failed: {:?}",
        result.error_message
    );
}

/// Test only cookies validation.
///
/// Checks if cookies are configured and valid.
#[tokio::test]
async fn smoke_test_cookies() {
    init_logging();

    let result = test_cookies_validation().await;

    println!("{}", result.format_log());

    // Skipped is OK (no cookies configured)
    assert!(
        result.status == SmokeTestStatus::Passed || result.status == SmokeTestStatus::Skipped,
        "Cookies validation failed: {:?}",
        result.error_message
    );
}

/// Test only metadata extraction.
///
/// Fetches metadata from the test YouTube video.
/// Note: Does NOT use cookies to avoid account bans.
#[tokio::test]
async fn smoke_test_metadata() {
    init_logging();

    let proxy_chain = get_test_proxy_chain();
    let result = test_metadata_extraction(DEFAULT_TEST_URL, Duration::from_secs(120), &proxy_chain).await;

    println!("{}", result.format_log());

    if let Some(title) = &result.metadata_title {
        println!("Extracted title: {}", title);
    }
    if let Some(proxy) = &result.proxy_used {
        println!("Proxy used: {}", proxy);
    }

    assert!(
        result.status == SmokeTestStatus::Passed,
        "Metadata extraction failed: {:?}",
        result.error_message
    );
}

/// Test only audio download (MP3).
///
/// Downloads the test video as MP3 and validates the file.
/// This test is marked as ignored because it performs real network operations.
#[tokio::test]
#[ignore]
async fn smoke_test_audio_only() {
    init_logging();

    let temp_dir = std::env::temp_dir().to_string_lossy().to_string();
    let proxy_chain = get_test_proxy_chain();

    let result = test_audio_download(DEFAULT_TEST_URL, &temp_dir, Duration::from_secs(180), &proxy_chain).await;

    println!("{}", result.format_log());

    if let Some(size) = result.file_size_bytes {
        println!("Audio file size: {} bytes", size);
    }
    if let Some(duration) = result.media_duration_secs {
        println!("Audio duration: {} seconds", duration);
    }
    if let Some(proxy) = &result.proxy_used {
        println!("Proxy used: {}", proxy);
    }

    assert!(
        result.status == SmokeTestStatus::Passed,
        "Audio download failed: {:?}",
        result.error_message
    );
}

/// Test only video download (MP4).
///
/// Downloads the test video as MP4 and validates the file.
/// This test is marked as ignored because it performs real network operations.
#[tokio::test]
#[ignore]
async fn smoke_test_video_only() {
    init_logging();

    let temp_dir = std::env::temp_dir().to_string_lossy().to_string();
    let proxy_chain = get_test_proxy_chain();

    let result = test_video_download(DEFAULT_TEST_URL, &temp_dir, Duration::from_secs(180), &proxy_chain).await;

    println!("{}", result.format_log());

    if let Some(size) = result.file_size_bytes {
        println!("Video file size: {} bytes", size);
    }
    if let Some(duration) = result.media_duration_secs {
        println!("Video duration: {} seconds", duration);
    }
    if let Some(has_streams) = result.video_has_both_streams {
        println!("Has video+audio streams: {}", has_streams);
    }
    if let Some(proxy) = &result.proxy_used {
        println!("Proxy used: {}", proxy);
    }

    assert!(
        result.status == SmokeTestStatus::Passed,
        "Video download failed: {:?}",
        result.error_message
    );
}

/// Quick smoke test for CI that only checks toolchain.
///
/// This is useful for fast CI pipelines where full download tests
/// might take too long.
#[tokio::test]
async fn smoke_test_quick() {
    init_logging();

    // Check tools are available
    assert!(is_ffmpeg_available(), "ffmpeg not found");
    assert!(is_ffprobe_available(), "ffprobe not found");
    assert!(is_ytdlp_available(), "yt-dlp not found");

    println!("Quick smoke test passed: all tools available");
}
