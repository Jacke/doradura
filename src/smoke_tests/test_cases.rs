//! Individual smoke test cases.
//!
//! Each test verifies a specific aspect of the bot's functionality:
//! - `test_ffmpeg_toolchain`: Checks that ffmpeg, ffprobe, and yt-dlp are available
//! - `test_cookies_validation`: Validates the cookies file format
//! - `test_metadata_extraction`: Tests yt-dlp metadata fetching
//! - `test_audio_download`: Downloads and validates an MP3 file
//! - `test_video_download`: Downloads and validates an MP4 file
//!
//! **Important**: Smoke tests do NOT use personal cookies to avoid account bans.
//! They use the v5.0 strategy: `android_vr,web_safari` clients + Deno runtime.

use super::results::SmokeTestResult;
use super::validators::{
    is_ffmpeg_available, is_ffprobe_available, is_ytdlp_available, validate_audio_file, validate_video_file,
};
use crate::core::config;
use crate::download::metadata::{validate_cookies_file_format, ProxyConfig};
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::time::timeout;

/// Adds yt-dlp arguments for smoke tests (v5.0 strategy).
///
/// Unlike production downloads, smoke tests do NOT use personal cookies
/// to avoid risking account bans from hourly health checks.
///
/// Uses:
/// - `android_vr,web_safari` player clients (no cookies/PO tokens needed)
/// - Deno JS runtime for YouTube challenge solving (yt-dlp 2026+)
/// - Proxy (if provided)
/// - NO cookies
fn add_smoke_test_args(args: &mut Vec<String>, proxy: Option<&ProxyConfig>) {
    // Add proxy if provided
    if let Some(proxy_config) = proxy {
        log::info!(
            "[smoke_test] Using proxy [{}]: {}",
            proxy_config.name,
            proxy_config.masked_url()
        );
        args.push("--proxy".to_string());
        args.push(proxy_config.url.clone());
    } else {
        log::info!("[smoke_test] No proxy, using direct connection");
    }

    // v5.0: Use android_vr + web_safari clients (don't require cookies or PO tokens)
    args.push("--extractor-args".to_string());
    args.push("youtube:player_client=android_vr,web,web_safari".to_string());

    // Use Deno JS runtime for YouTube challenge solving (yt-dlp 2026+)
    args.push("--js-runtimes".to_string());
    args.push("deno".to_string());

    // NO cookies - smoke tests should not use personal accounts
    // This avoids risking account bans from hourly health checks
}

/// Test 1: Verify FFmpeg toolchain is available.
///
/// Checks that the following tools are installed and accessible:
/// - ffmpeg (for audio/video conversion)
/// - ffprobe (for media file validation)
/// - yt-dlp (for downloading from YouTube)
pub async fn test_ffmpeg_toolchain() -> SmokeTestResult {
    let start = Instant::now();
    let test_name = "ffmpeg_toolchain";

    // Check ffmpeg
    if !is_ffmpeg_available() {
        return SmokeTestResult::failed(test_name, start.elapsed(), "ffmpeg not found in PATH");
    }

    // Check ffprobe
    if !is_ffprobe_available() {
        return SmokeTestResult::failed(test_name, start.elapsed(), "ffprobe not found in PATH");
    }

    // Check yt-dlp
    if !is_ytdlp_available() {
        return SmokeTestResult::failed(test_name, start.elapsed(), "yt-dlp not found in PATH");
    }

    SmokeTestResult::passed(test_name, start.elapsed())
}

/// Test 2: Validate cookies file format.
///
/// Checks that the configured cookies file exists and has valid Netscape format.
/// If no cookies file is configured, the test passes with a warning.
pub async fn test_cookies_validation() -> SmokeTestResult {
    let start = Instant::now();
    let test_name = "cookies_validation";

    // Check if cookies file is configured
    if let Some(ref cookies_file) = *config::YTDL_COOKIES_FILE {
        if !cookies_file.is_empty() {
            // Expand path
            let expanded = shellexpand::tilde(cookies_file);
            let cookies_path = expanded.to_string();

            // Check file exists
            if !Path::new(&cookies_path).exists() {
                return SmokeTestResult::failed(
                    test_name,
                    start.elapsed(),
                    &format!("Cookies file not found: {}", cookies_path),
                );
            }

            // Validate format
            if !validate_cookies_file_format(&cookies_path) {
                return SmokeTestResult::failed(test_name, start.elapsed(), "Cookies file has invalid Netscape format");
            }

            return SmokeTestResult::passed(test_name, start.elapsed());
        }
    }

    // Check browser cookies config
    let browser = config::YTDL_COOKIES_BROWSER.as_str();
    if !browser.is_empty() {
        // Browser cookies configured - can't validate directly
        let mut result = SmokeTestResult::passed(test_name, start.elapsed());
        result.error_message = Some(format!("Using browser cookies from: {}", browser));
        return result;
    }

    // No cookies configured
    SmokeTestResult::skipped(
        test_name,
        "No cookies configured (YTDL_COOKIES_FILE or YTDL_COOKIES_BROWSER)",
    )
}

/// Test 3: Extract metadata from YouTube video.
///
/// Uses yt-dlp to fetch title and artist metadata from the test URL.
/// This verifies that yt-dlp can communicate with YouTube successfully.
///
/// **Note**: Does NOT use cookies to avoid account bans.
pub async fn test_metadata_extraction(
    test_url: &str,
    test_timeout: Duration,
    proxy_chain: &[Option<ProxyConfig>],
) -> SmokeTestResult {
    let start = Instant::now();
    let test_name = "metadata_extraction";

    let ytdl_bin = &*config::YTDL_BIN;

    // Try each proxy in chain
    for (idx, proxy) in proxy_chain.iter().enumerate() {
        let proxy_name = proxy.as_ref().map(|p| p.name.as_str()).unwrap_or("direct");
        log::debug!("Trying metadata extraction with proxy [{}]: {}", idx, proxy_name);

        // Build arguments
        let mut args: Vec<String> = vec![
            "--print".to_string(),
            "%(title)s".to_string(),
            "--no-playlist".to_string(),
            "--skip-download".to_string(),
        ];

        // Add proxy + PO Token (NO cookies)
        add_smoke_test_args(&mut args, proxy.as_ref());

        args.push("--no-check-certificate".to_string());
        args.push(test_url.to_string());

        let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

        // Run with timeout
        let result = timeout(
            test_timeout,
            tokio::process::Command::new(ytdl_bin).args(&args_refs).output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                if output.status.success() {
                    let title = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if title.is_empty() {
                        log::warn!("yt-dlp returned empty title with proxy {}", proxy_name);
                        continue;
                    }

                    let mut result = SmokeTestResult::passed(test_name, start.elapsed());
                    result.metadata_title = Some(title);
                    result.proxy_used = Some(proxy_name.to_string());
                    return result;
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    log::warn!(
                        "yt-dlp metadata failed with proxy {}: {}",
                        proxy_name,
                        &stderr[..stderr.len().min(200)]
                    );
                    continue;
                }
            }
            Ok(Err(e)) => {
                log::warn!("Failed to run yt-dlp with proxy {}: {}", proxy_name, e);
                continue;
            }
            Err(_) => {
                return SmokeTestResult::timeout(test_name, test_timeout);
            }
        }
    }

    SmokeTestResult::failed(test_name, start.elapsed(), "All proxies failed for metadata extraction")
}

/// Test 4: Download audio file (MP3).
///
/// Downloads the test video as MP3 and validates:
/// - File size is reasonable (>10KB)
/// - File has valid audio duration
/// - File can be probed by ffprobe
pub async fn test_audio_download(
    test_url: &str,
    temp_dir: &str,
    test_timeout: Duration,
    proxy_chain: &[Option<ProxyConfig>],
) -> SmokeTestResult {
    let start = Instant::now();
    let test_name = "audio_download";

    let ytdl_bin = &*config::YTDL_BIN;
    let timestamp = chrono::Utc::now().timestamp();
    let output_path = format!("{}/smoke_test_audio_{}.mp3", temp_dir, timestamp);

    // Try each proxy in chain
    for (idx, proxy) in proxy_chain.iter().enumerate() {
        let proxy_name = proxy.as_ref().map(|p| p.name.as_str()).unwrap_or("direct");
        log::debug!("Trying audio download with proxy [{}]: {}", idx, proxy_name);

        // Build arguments
        let mut args: Vec<String> = vec![
            "-x".to_string(),
            "--audio-format".to_string(),
            "mp3".to_string(),
            "--audio-quality".to_string(),
            "320K".to_string(),
            "-o".to_string(),
            output_path.clone(),
            "--no-playlist".to_string(),
        ];

        // Add proxy + PO Token (NO cookies)
        add_smoke_test_args(&mut args, proxy.as_ref());

        args.push("--no-check-certificate".to_string());
        args.push(test_url.to_string());

        let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

        // Run with timeout
        let result = timeout(
            test_timeout,
            tokio::process::Command::new(ytdl_bin).args(&args_refs).output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                if output.status.success() {
                    // Validate the downloaded file
                    let validation = validate_audio_file(Path::new(&output_path));

                    // Cleanup
                    let _ = std::fs::remove_file(&output_path);

                    if validation.is_valid {
                        let mut result = SmokeTestResult::passed(test_name, start.elapsed());
                        result.file_size_bytes = Some(validation.size);
                        result.media_duration_secs = validation.duration;
                        result.proxy_used = Some(proxy_name.to_string());
                        return result;
                    } else {
                        // File invalid, try next proxy
                        log::warn!(
                            "Audio file validation failed with proxy {}: {:?}",
                            proxy_name,
                            validation.error
                        );
                        continue;
                    }
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    log::warn!(
                        "yt-dlp failed with proxy {}: {}",
                        proxy_name,
                        &stderr[..stderr.len().min(200)]
                    );
                    // Try next proxy
                    continue;
                }
            }
            Ok(Err(e)) => {
                log::warn!("Failed to run yt-dlp with proxy {}: {}", proxy_name, e);
                continue;
            }
            Err(_) => {
                // Timeout - don't try more proxies
                let _ = std::fs::remove_file(&output_path);
                return SmokeTestResult::timeout(test_name, test_timeout);
            }
        }
    }

    // All proxies failed
    let _ = std::fs::remove_file(&output_path);
    SmokeTestResult::failed(test_name, start.elapsed(), "All proxies failed for audio download")
}

/// Test 5: Download video file (MP4).
///
/// Downloads the test video as MP4 and validates:
/// - File has video stream
/// - File has audio stream
/// - File has valid duration
pub async fn test_video_download(
    test_url: &str,
    temp_dir: &str,
    test_timeout: Duration,
    proxy_chain: &[Option<ProxyConfig>],
) -> SmokeTestResult {
    let start = Instant::now();
    let test_name = "video_download";

    let ytdl_bin = &*config::YTDL_BIN;
    let timestamp = chrono::Utc::now().timestamp();
    let output_path = format!("{}/smoke_test_video_{}.mp4", temp_dir, timestamp);

    // Try each proxy in chain
    for (idx, proxy) in proxy_chain.iter().enumerate() {
        let proxy_name = proxy.as_ref().map(|p| p.name.as_str()).unwrap_or("direct");
        log::debug!("Trying video download with proxy [{}]: {}", idx, proxy_name);

        // Build arguments - use small format for faster test
        let mut args: Vec<String> = vec![
            "-f".to_string(),
            "worst[ext=mp4]/worst".to_string(), // Use worst quality for speed
            "-o".to_string(),
            output_path.clone(),
            "--no-playlist".to_string(),
        ];

        // Add proxy + PO Token (NO cookies)
        add_smoke_test_args(&mut args, proxy.as_ref());

        args.push("--no-check-certificate".to_string());
        args.push(test_url.to_string());

        let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

        // Run with timeout
        let result = timeout(
            test_timeout,
            tokio::process::Command::new(ytdl_bin).args(&args_refs).output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                if output.status.success() {
                    // Validate the downloaded file
                    let validation = validate_video_file(Path::new(&output_path));

                    // Cleanup
                    let _ = std::fs::remove_file(&output_path);

                    if validation.is_valid {
                        let mut result = SmokeTestResult::passed(test_name, start.elapsed());
                        result.file_size_bytes = Some(validation.size);
                        result.media_duration_secs = validation.duration;
                        result.video_has_both_streams =
                            Some(validation.has_video_stream && validation.has_audio_stream);
                        result.proxy_used = Some(proxy_name.to_string());
                        return result;
                    } else {
                        log::warn!(
                            "Video file validation failed with proxy {}: {:?}",
                            proxy_name,
                            validation.error
                        );
                        continue;
                    }
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    log::warn!(
                        "yt-dlp failed with proxy {}: {}",
                        proxy_name,
                        &stderr[..stderr.len().min(200)]
                    );
                    continue;
                }
            }
            Ok(Err(e)) => {
                log::warn!("Failed to run yt-dlp with proxy {}: {}", proxy_name, e);
                continue;
            }
            Err(_) => {
                let _ = std::fs::remove_file(&output_path);
                return SmokeTestResult::timeout(test_name, test_timeout);
            }
        }
    }

    // All proxies failed
    let _ = std::fs::remove_file(&output_path);
    SmokeTestResult::failed(test_name, start.elapsed(), "All proxies failed for video download")
}

#[cfg(test)]
mod tests {
    use super::super::results::SmokeTestStatus;
    use super::*;

    #[tokio::test]
    async fn test_ffmpeg_toolchain_available() {
        let result = test_ffmpeg_toolchain().await;
        // This test depends on the environment, so we just check the structure
        assert!(!result.test_name.is_empty());
        assert!(matches!(
            result.status,
            SmokeTestStatus::Passed | SmokeTestStatus::Failed
        ));
    }

    #[tokio::test]
    async fn test_cookies_validation_runs() {
        let result = test_cookies_validation().await;
        assert!(!result.test_name.is_empty());
        // Can be Passed, Skipped, or Failed depending on config
    }
}
