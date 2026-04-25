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

use doradura::smoke_tests::{
    DEFAULT_TEST_URL, ODESLI_TEST_URL, SmokeTestConfig, SmokeTestStatus, is_ffmpeg_available, is_ffprobe_available,
    is_ytdlp_available, run_all_smoke_tests, test_audio_download, test_cookies_validation, test_ffmpeg_toolchain,
    test_metadata_extraction, test_odesli_fetch, test_video_download, test_web_server_health,
};
use std::time::Duration;

/// Initialize logging for tests
fn init_logging() {
    let _ = pretty_env_logger::try_init();
}

/// Helper to get proxy chain for tests
fn get_test_proxy_chain() -> Vec<Option<doradura::download::metadata::ProxyConfig>> {
    doradura::smoke_tests::runner::get_smoke_test_proxy_chain()
}

/// Full smoke test suite - runs all 5 tests.
///
/// This is the main entry point for CI. It runs all smoke tests and
/// verifies that the bot can download audio and video from YouTube.
///
/// Note: In CI without proxy/PO Token configured, download tests will be
/// skipped (not failed) - this is expected behavior.
#[tokio::test]
async fn smoke_test_full_suite() {
    init_logging();

    let config = SmokeTestConfig::for_ci();
    let report = run_all_smoke_tests(&config).await;

    println!("\n{}\n", report.format_log());

    // Check for critical failures (not proxy/external-API-related)
    // Proxy failures are expected in CI without authentication setup.
    // External API failures (lyrics, Odesli, web server) are soft failures — the
    // bot should not break if these third-party services are temporarily down.
    let has_critical_failures = report.results.iter().any(|r| {
        if !matches!(r.status, SmokeTestStatus::Failed | SmokeTestStatus::Timeout) {
            return false;
        }
        let test_name = r.test_name.as_str();
        let error_msg = r.error_message.as_deref().unwrap_or("");

        // Allow proxy-related failures in CI (expected without auth)
        let is_proxy_error = error_msg.contains("All proxies failed")
            || error_msg.contains("403")
            || error_msg.contains("Forbidden")
            || error_msg.contains("bot")
            || error_msg.contains("sign in");

        // Allow external-API-dependent tests to fail (LRCLIB, Odesli, web server)
        // These depend on third-party services that may be temporarily unavailable.
        let is_external_api_test = matches!(test_name, "lyrics_fetch" | "odesli_fetch" | "web_server_health");

        // Only fail on non-proxy, non-external-API errors (e.g., ffmpeg not found)
        !is_proxy_error && !is_external_api_test
    });

    if has_critical_failures {
        for result in &report.results {
            if matches!(result.status, SmokeTestStatus::Failed | SmokeTestStatus::Timeout) {
                let error_msg = result.error_message.as_deref().unwrap_or("unknown");
                eprintln!("FAILED: {} - {}", result.test_name, error_msg);
            }
        }
        panic!(
            "Smoke tests failed with critical errors: {}/{} passed",
            report.passed_count,
            report.results.len()
        );
    }

    // Count proxy-related skips for logging
    let proxy_skipped = report
        .results
        .iter()
        .filter(|r| {
            matches!(r.status, SmokeTestStatus::Failed)
                && r.error_message.as_deref().unwrap_or("").contains("All proxies failed")
        })
        .count();

    // Count external-API soft failures for logging
    let api_soft_failed = report
        .results
        .iter()
        .filter(|r| {
            matches!(r.status, SmokeTestStatus::Failed | SmokeTestStatus::Timeout)
                && matches!(
                    r.test_name.as_str(),
                    "lyrics_fetch" | "odesli_fetch" | "web_server_health"
                )
        })
        .count();

    if proxy_skipped > 0 {
        println!(
            "⚠️  {} download test(s) skipped due to no proxy/auth configured (expected in CI)",
            proxy_skipped
        );
    }
    if api_soft_failed > 0 {
        println!(
            "⚠️  {} external-API test(s) soft-failed (LRCLIB/Odesli/web server may be temporarily unavailable)",
            api_soft_failed
        );
    }

    println!(
        "Smoke tests completed: {}/{} passed ({} proxy-skipped, {} api-soft-failed)",
        report.passed_count,
        report.results.len(),
        proxy_skipped,
        api_soft_failed,
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
/// Note: In CI without proxy/PO Token, this test will be skipped.
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

    // Allow proxy-related failures in CI (expected without auth)
    let error_msg = result.error_message.as_deref().unwrap_or("");
    let is_proxy_error =
        error_msg.contains("All proxies failed") || error_msg.contains("403") || error_msg.contains("Forbidden");

    if result.status == SmokeTestStatus::Failed && is_proxy_error {
        println!("⚠️  Metadata test skipped: no proxy/auth configured (expected in CI)");
        return;
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

/// Test 8: Odesli streaming links.
///
/// Calls api.song.link with the test YouTube URL and checks at least one
/// streaming platform link is returned. Requires network access.
#[tokio::test]
async fn smoke_test_odesli() {
    init_logging();

    let result = test_odesli_fetch(ODESLI_TEST_URL).await;

    println!("{}", result.format_log());

    if let Some(ref info) = result.metadata_title {
        println!("Streaming platforms: {}", info);
    }

    // Allow failure if Odesli is down (not a critical dependency)
    match result.status {
        SmokeTestStatus::Passed => println!("Odesli OK"),
        SmokeTestStatus::Timeout => println!("⚠️  Odesli timed out (API may be slow)"),
        SmokeTestStatus::Failed => println!(
            "⚠️  Odesli returned no links: {}",
            result.error_message.as_deref().unwrap_or("unknown")
        ),
        _ => {}
    }
}

/// Test 9: Web server health check.
///
/// Hits /health and /api/s/{nonexistent} on WEB_BASE_URL.
/// Skipped automatically if WEB_BASE_URL is not set.
#[tokio::test]
async fn smoke_test_web_server() {
    init_logging();

    let result = test_web_server_health().await;

    println!("{}", result.format_log());

    if let Some(ref url) = result.metadata_title {
        println!("Web server URL: {}", url);
    }

    assert!(
        matches!(result.status, SmokeTestStatus::Passed | SmokeTestStatus::Skipped),
        "Web server health check failed: {:?}",
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
