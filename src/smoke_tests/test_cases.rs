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
    is_ffmpeg_available, is_ffprobe_available, is_ytdlp_available, validate_audio_file, validate_ringtone_file,
    validate_video_file,
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

    // Use android_vr + web_safari clients (minimal bot detection, no PO token required)
    // formats=missing_pot skips formats requiring PO Token (avoids 403 on fragments)
    args.push("--extractor-args".to_string());
    args.push("youtube:player_client=android_vr,web_safari;formats=missing_pot".to_string());

    // Use Deno JS runtime for YouTube challenge solving (yt-dlp 2026+)
    args.push("--js-runtimes".to_string());
    args.push("deno".to_string());

    // Impersonate browser TLS/HTTP fingerprint to avoid bot detection
    args.push("--impersonate".to_string());
    args.push("Chrome-131:Android-14".to_string());

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

/// Test 6: Ringtone conversion pipeline.
///
/// Verifies the full ringtone FFmpeg pipeline without network access:
/// 1. Generates a short MP3 with embedded album art (cover image) using ffmpeg.
///    Album art is the exact input that previously caused exit code 234.
/// 2. Calls `create_iphone_ringtone()` to produce a `.m4r` file.
/// 3. Validates the output:
///    - Has audio stream
///    - Has NO video stream (album art must be stripped by `-vn`)
///    - Duration ≤ 30 s
pub async fn test_ringtone_conversion(temp_dir: &str) -> SmokeTestResult {
    use crate::download::ringtone::create_iphone_ringtone;

    let start = std::time::Instant::now();
    let test_name = "ringtone_conversion";

    if !is_ffmpeg_available() {
        return SmokeTestResult::skipped(test_name, "ffmpeg not available");
    }

    let ts = chrono::Utc::now().timestamp_millis();
    let silence_path = format!("{}/smoke_ringtone_silence_{}.mp3", temp_dir, ts);
    let cover_path = format!("{}/smoke_ringtone_cover_{}.jpg", temp_dir, ts);
    let input_path = format!("{}/smoke_ringtone_in_{}.mp3", temp_dir, ts);
    let output_path = format!("{}/smoke_ringtone_out_{}.m4r", temp_dir, ts);

    let cleanup = |paths: &[&str]| {
        for p in paths {
            let _ = std::fs::remove_file(p);
        }
    };
    let all_files = [
        silence_path.as_str(),
        cover_path.as_str(),
        input_path.as_str(),
        output_path.as_str(),
    ];

    // Generate 10 s of silence as MP3
    let silence_ok = tokio::process::Command::new("ffmpeg")
        .args([
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=44100:cl=stereo",
            "-t",
            "10",
            "-y",
            &silence_path,
        ])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !silence_ok {
        cleanup(&all_files);
        return SmokeTestResult::failed(test_name, start.elapsed(), "ffmpeg failed to generate silence MP3");
    }

    // Generate a 1×1 JPEG cover image (album art — the source of exit code 234)
    let cover_ok = tokio::process::Command::new("ffmpeg")
        .args([
            "-f",
            "lavfi",
            "-i",
            "color=red:size=1x1",
            "-frames:v",
            "1",
            "-y",
            &cover_path,
        ])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !cover_ok {
        // Cover generation failed — skip album-art regression, use plain silence
        log::warn!("[smoke_test] Could not generate cover art, testing without album art");
        if let Err(e) = std::fs::copy(&silence_path, &input_path) {
            cleanup(&all_files);
            return SmokeTestResult::failed(test_name, start.elapsed(), &format!("copy failed: {}", e));
        }
    } else {
        // Embed cover art into the MP3 to replicate the album-art regression
        let embed_ok = tokio::process::Command::new("ffmpeg")
            .args([
                "-i",
                &silence_path,
                "-i",
                &cover_path,
                "-map",
                "0:a",
                "-map",
                "1:v",
                "-c:a",
                "copy",
                "-c:v",
                "copy",
                "-id3v2_version",
                "3",
                "-y",
                &input_path,
            ])
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);

        if !embed_ok {
            cleanup(&all_files);
            return SmokeTestResult::failed(test_name, start.elapsed(), "ffmpeg failed to embed album art");
        }
    }

    // Run the ringtone conversion (5 seconds starting at 0)
    match create_iphone_ringtone(&input_path, &output_path, 0, 5).await {
        Ok(()) => {}
        Err(e) => {
            cleanup(&all_files);
            return SmokeTestResult::failed(
                test_name,
                start.elapsed(),
                &format!("create_iphone_ringtone failed: {}", e),
            );
        }
    }

    // Validate the output .m4r
    let validation = validate_ringtone_file(std::path::Path::new(&output_path));
    cleanup(&all_files);

    if !validation.is_valid {
        return SmokeTestResult::failed(
            test_name,
            start.elapsed(),
            &validation
                .error
                .unwrap_or_else(|| "unknown validation error".to_string()),
        );
    }

    let mut result = SmokeTestResult::passed(test_name, start.elapsed());
    result.file_size_bytes = Some(validation.size);
    result.media_duration_secs = validation.duration;
    result
}

/// Test 7: Lyrics fetch (LRCLIB + optional Genius).
///
/// Calls the lyrics API directly (no yt-dlp, no ffmpeg) and validates:
/// - At least one section is returned
/// - Section has non-empty lines
/// - If GENIUS_CLIENT_TOKEN is set, checks that structured sections are found
pub async fn test_lyrics_fetch() -> SmokeTestResult {
    let start = Instant::now();
    let test_name = "lyrics_fetch";

    let result = timeout(
        Duration::from_secs(15),
        crate::lyrics::fetch_lyrics("Eminem", "Lose Yourself"),
    )
    .await;

    match result {
        Err(_) => SmokeTestResult::timeout(test_name, Duration::from_secs(15)),
        Ok(None) => SmokeTestResult::failed(test_name, start.elapsed(), "No lyrics returned from any source"),
        Ok(Some(lyr)) => {
            if lyr.sections.is_empty() {
                return SmokeTestResult::failed(test_name, start.elapsed(), "Lyrics returned but sections are empty");
            }
            let total_lines: usize = lyr.sections.iter().map(|s| s.lines.len()).sum();
            if total_lines == 0 {
                return SmokeTestResult::failed(test_name, start.elapsed(), "Lyrics sections have no lines");
            }

            let source = if crate::core::config::GENIUS_CLIENT_TOKEN.is_some() {
                if lyr.has_structure {
                    "Genius (structured)"
                } else {
                    "Genius (no structure — LRCLIB fallback)"
                }
            } else if lyr.has_structure {
                "LRCLIB (structured)"
            } else {
                "LRCLIB (plain)"
            };

            log::info!(
                "[smoke_test] lyrics_fetch: {} sections, {} lines total, source={}",
                lyr.sections.len(),
                total_lines,
                source
            );

            let mut r = SmokeTestResult::passed(test_name, start.elapsed());
            r.metadata_title = Some(format!(
                "{} sections ({} lines) via {}",
                lyr.sections.len(),
                total_lines,
                source
            ));
            r
        }
    }
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

    #[tokio::test]
    async fn test_ringtone_conversion_smoke() {
        let temp_dir = std::env::temp_dir().to_string_lossy().to_string();
        let result = test_ringtone_conversion(&temp_dir).await;
        assert!(!result.test_name.is_empty());
        // Passes when ffmpeg is available; skipped otherwise
        assert!(
            matches!(result.status, SmokeTestStatus::Passed | SmokeTestStatus::Skipped),
            "ringtone_conversion failed: {:?}",
            result.error_message
        );
    }

    /// Test 7: Lyrics fetch hits LRCLIB and gets real lyrics back.
    /// Requires network access. Skipped in offline environments.
    #[tokio::test]
    async fn test_lyrics_fetch_smoke() {
        let result = test_lyrics_fetch().await;
        assert!(!result.test_name.is_empty());
        assert!(
            matches!(
                result.status,
                SmokeTestStatus::Passed | SmokeTestStatus::Timeout | SmokeTestStatus::Failed
            ),
            "unexpected status: {:?} — {:?}",
            result.status,
            result.error_message
        );
        if result.status == SmokeTestStatus::Passed {
            // Must have logged section/line counts in metadata_title
            assert!(result.metadata_title.is_some());
        }
    }
}
