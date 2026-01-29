//! Video/audio metadata extraction and probing utilities.
//!
//! This module provides functions for extracting metadata from media files
//! using ffprobe and yt-dlp. It includes:
//!
//! - Duration and dimension probing via ffprobe
//! - Stream detection (video/audio presence)
//! - yt-dlp metadata fetching (title, artist)
//! - Cookie handling for authenticated downloads
//! - Telegram-compatible format string generation

use crate::core::config;
use crate::core::error::AppError;
use crate::core::metrics;
use crate::download::ytdlp_errors::{analyze_ytdlp_error, get_error_message, should_notify_admin, YtDlpErrorType};
use crate::storage::cache;
use crate::telegram::notifications::notify_admin_text;
use crate::telegram::Bot;
use std::fs;
use std::path::Path;
use std::process::Command;
use teloxide::prelude::*;
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;
use url::Url;

/// Masks password in proxy URL for safe logging.
/// Transforms "http://user:secret@host:port" to "http://user:***@host:port"
pub fn mask_proxy_password(proxy_url: &str) -> String {
    if let Some(at_pos) = proxy_url.rfind('@') {
        if let Some(colon_pos) = proxy_url[..at_pos].rfind(':') {
            let prefix = &proxy_url[..colon_pos + 1];
            let suffix = &proxy_url[at_pos..];
            return format!("{}***{}", prefix, suffix);
        }
    }
    proxy_url.to_string()
}

/// Represents a proxy configuration with URL and description for logging
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    /// Proxy URL (e.g., "socks5://host:port" or "http://user:pass@host:port")
    pub url: String,
    /// Human-readable description for logs (e.g., "WARP", "Geonode residential")
    pub name: String,
}

impl ProxyConfig {
    pub fn new(url: String, name: &str) -> Self {
        Self {
            url,
            name: name.to_string(),
        }
    }

    /// Returns masked URL for safe logging
    pub fn masked_url(&self) -> String {
        mask_proxy_password(&self.url)
    }
}

/// Returns ordered list of proxies to try: WARP (primary) → PROXY_LIST (fallback) → None (direct)
///
/// This enables automatic failover when a proxy fails:
/// 1. First try WARP proxy (free Cloudflare IP)
/// 2. If WARP fails, try residential proxy from PROXY_LIST
/// 3. If all proxies fail, try direct connection (no proxy)
pub fn get_proxy_chain() -> Vec<Option<ProxyConfig>> {
    let mut chain = Vec::new();

    // Primary: WARP proxy (free Cloudflare)
    if let Some(ref warp_proxy) = *config::proxy::WARP_PROXY {
        if !warp_proxy.trim().is_empty() {
            chain.push(Some(ProxyConfig::new(
                warp_proxy.trim().to_string(),
                "WARP (Cloudflare)",
            )));
        }
    }

    // Fallback: Residential proxy from PROXY_LIST
    if let Some(ref proxy_list) = *config::proxy::PROXY_LIST {
        let first_proxy = proxy_list.split(',').next().unwrap_or("").trim();
        if !first_proxy.is_empty() {
            chain.push(Some(ProxyConfig::new(
                first_proxy.to_string(),
                "Residential (fallback)",
            )));
        }
    }

    // Last resort: No proxy (direct connection)
    chain.push(None);

    chain
}

/// Checks if an error is proxy-related and should trigger fallback to next proxy
pub fn is_proxy_related_error(error_msg: &str) -> bool {
    let error_lower = error_msg.to_lowercase();
    error_lower.contains("403")
        || error_lower.contains("forbidden")
        || error_lower.contains("proxy")
        || error_lower.contains("connection")
        || error_lower.contains("timeout")
        || error_lower.contains("bot")
        || error_lower.contains("sign in")
        || error_lower.contains("confirm you")
        || error_lower.contains("socks")
        || error_lower.contains("tunnel")
}

/// Validates Netscape HTTP Cookie File format.
///
/// The Netscape format starts with "# Netscape HTTP Cookie File" or "# HTTP Cookie File"
/// and contains lines in the format: domain\tflag\tpath\tsecure\texpiration\tname\tvalue
///
/// # Arguments
///
/// * `cookies_file` - Path to the cookies file to validate
///
/// # Returns
///
/// `true` if the file exists and has valid Netscape format, `false` otherwise
pub fn validate_cookies_file_format(cookies_file: &str) -> bool {
    if let Ok(contents) = std::fs::read_to_string(cookies_file) {
        // Check for Netscape header
        let has_header = contents.lines().any(|line| {
            line.trim().starts_with("# Netscape HTTP Cookie File") || line.trim().starts_with("# HTTP Cookie File")
        });

        // Check for at least one cookie line (format: domain\tflag\tpath...)
        let has_cookies = contents.lines().any(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#') && trimmed.split('\t').count() >= 7
        });

        has_header && has_cookies
    } else {
        false
    }
}

/// Adds proxy, cookie, and PO Token arguments to yt-dlp command arguments.
///
/// This is a convenience wrapper that uses the default proxy chain (WARP → residential → direct).
/// For retry logic with specific proxy, use `add_cookies_args_with_proxy` instead.
pub fn add_cookies_args(args: &mut Vec<&str>) {
    // Use first available proxy from the chain (WARP or PROXY_LIST)
    let proxy_chain = get_proxy_chain();
    let first_proxy = proxy_chain.into_iter().find(|p| p.is_some()).flatten();
    add_cookies_args_with_proxy(args, first_proxy.as_ref());
}

/// Adds proxy, cookie, and PO Token arguments with a specific proxy configuration.
///
/// # Arguments
///
/// * `args` - Vector of arguments for yt-dlp to modify
/// * `proxy` - Optional proxy configuration. If None, no proxy is used (direct connection)
///
/// # Note
///
/// This function uses `Box::leak` to create static string references for the cookies
/// path. This is intentional for lifetime purposes in the yt-dlp argument handling.
pub fn add_cookies_args_with_proxy(args: &mut Vec<&str>, proxy: Option<&ProxyConfig>) {
    // Add proxy if provided
    if let Some(proxy_config) = proxy {
        log::info!("Using proxy [{}]: {}", proxy_config.name, proxy_config.masked_url());
        args.push("--proxy");
        // SAFETY: This reference lives long enough as it's from Box::leak
        let leaked_proxy = Box::leak(proxy_config.url.clone().into_boxed_str());
        args.push(unsafe { std::mem::transmute::<&str, &'static str>(leaked_proxy) });
    } else {
        log::info!("No proxy configured, using direct connection");
    }

    // Add PO Token provider configuration for YouTube
    // bgutil HTTP server runs on port 4416 by default
    args.push("--extractor-args");
    args.push("youtubepot-bgutilhttp:base_url=http://127.0.0.1:4416");

    // Priority 1: Cookies file
    if let Some(ref cookies_file) = *config::YTDL_COOKIES_FILE {
        if !cookies_file.is_empty() {
            // Convert relative path to absolute (if needed)
            let cookies_path = if std::path::Path::new(cookies_file).is_absolute() {
                cookies_file.clone()
            } else {
                // Try to find file in current directory or expand tilde
                let expanded = shellexpand::tilde(cookies_file);
                expanded.to_string()
            };

            // Check file existence
            let cookies_path_buf = std::path::Path::new(&cookies_path);
            if !cookies_path_buf.exists() {
                log::error!("Cookies file not found: {} (checked: {})", cookies_file, cookies_path);
                log::error!("   Current working directory: {:?}", std::env::current_dir());
                log::error!("   YouTube downloads will FAIL without valid cookies!");
                log::error!("   Please check the path and ensure the file exists.");
                // Don't add cookies arguments if file not found
                return;
            } else {
                // Get absolute path for logging
                let abs_path = cookies_path_buf
                    .canonicalize()
                    .unwrap_or_else(|_| cookies_path_buf.to_path_buf());

                // Validate file format
                if !validate_cookies_file_format(&cookies_path) {
                    log::warn!("Cookies file format may be invalid: {}", abs_path.display());
                    log::warn!("Expected Netscape HTTP Cookie File format:");
                    log::warn!("  - Header: # Netscape HTTP Cookie File");
                    log::warn!("  - Format: domain\\tflag\\tpath\\tsecure\\texpiration\\tname\\tvalue");
                    log::warn!("See: https://github.com/yt-dlp/yt-dlp/wiki/FAQ#how-do-i-pass-cookies-to-yt-dlp");
                    log::warn!("You may need to re-export cookies from your browser.");
                } else {
                    log::info!("Cookies file format validated: {}", abs_path.display());
                }

                args.push("--cookies");
                // Use absolute path for reliability
                let abs_path_str = abs_path.to_string_lossy().to_string();
                // SAFETY: This reference lives long enough as it's from Box::leak
                let leaked_path = Box::leak(abs_path_str.into_boxed_str());
                args.push(unsafe { std::mem::transmute::<&str, &'static str>(leaked_path) });
                log::info!("Using cookies from file: {}", abs_path.display());
                return;
            }
        }
    }

    // Priority 2: Browser
    let browser = config::YTDL_COOKIES_BROWSER.as_str();
    if !browser.is_empty() {
        args.push("--cookies-from-browser");
        args.push(browser);
        log::info!("Using cookies from browser: {}", browser);
    } else {
        log::warn!("-----------------------------------------------------------");
        log::warn!("NO COOKIES CONFIGURED!");
        log::warn!("-----------------------------------------------------------");
        log::warn!("YouTube downloads will fail with 'bot detection' or 'only images' errors!");
        log::warn!("");

        #[cfg(target_os = "macos")]
        {
            log::warn!("macOS USERS:");
            log::warn!("   Browser cookie extraction requires Full Disk Access.");
            log::warn!("   It's MUCH EASIER to export cookies to a file!");
            log::warn!("");
            log::warn!("   See: MACOS_COOKIES_FIX.md for step-by-step guide");
            log::warn!("");
            log::warn!("   Quick fix:");
            log::warn!("   1. Install Chrome extension: Get cookies.txt LOCALLY");
            log::warn!("   2. Go to youtube.com -> login");
            log::warn!("   3. Click extension -> Export -> save as youtube_cookies.txt");
            log::warn!("   4. Run: ./scripts/run_with_cookies.sh");
        }

        #[cfg(not(target_os = "macos"))]
        {
            log::warn!("AUTOMATIC COOKIE EXTRACTION (Recommended):");
            log::warn!("   1. Login to YouTube in your browser (chrome/firefox/etc)");
            log::warn!("   2. Install dependencies: pip3 install keyring pycryptodomex");
            log::warn!("   3. Set browser: export YTDL_COOKIES_BROWSER=chrome");
            log::warn!("      Supported: chrome, firefox, safari, brave, chromium, edge, opera, vivaldi");
            log::warn!("   4. Restart the bot");
            log::warn!("");
            log::warn!("OR EXPORT TO FILE (Alternative):");
            log::warn!("   1. Export cookies from browser to youtube_cookies.txt");
            log::warn!("   2. Set: export YTDL_COOKIES_FILE=youtube_cookies.txt");
            log::warn!("   3. Restart the bot");
        }

        log::warn!("-----------------------------------------------------------");
    }
}

/// Probes media file duration using ffprobe.
///
/// # Arguments
///
/// * `path` - Path to the media file
///
/// # Returns
///
/// Duration in seconds if successful, `None` otherwise
pub fn probe_duration_seconds(path: &str) -> Option<u32> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
        .ok()?;

    let duration_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if duration_str.is_empty() {
        return None;
    }
    let secs = duration_str.parse::<f32>().ok()?;
    Some(secs.round() as u32)
}

/// Checks if a media file contains both video and audio streams.
///
/// # Arguments
///
/// * `path` - Path to the media file
///
/// # Returns
///
/// `true` if the file has both video and audio tracks, `false` otherwise
pub fn has_both_video_and_audio(path: &str) -> Result<bool, AppError> {
    // Check for video stream
    let video_output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_type",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
        .map_err(|e| AppError::Download(format!("Failed to check video stream: {}", e)))?;

    let has_video = !String::from_utf8_lossy(&video_output.stdout).trim().is_empty();

    // Check for audio stream
    let audio_output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_type",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
        .map_err(|e| AppError::Download(format!("Failed to check audio stream: {}", e)))?;

    let has_audio = !String::from_utf8_lossy(&audio_output.stdout).trim().is_empty();

    Ok(has_video && has_audio)
}

/// Probes video metadata: duration, width, and height.
///
/// Used for correctly sending videos to Telegram which requires these parameters.
///
/// # Arguments
///
/// * `path` - Path to the video file
///
/// # Returns
///
/// Tuple of (duration_seconds, width, height) if successful
pub fn probe_video_metadata(path: &str) -> Option<(u32, Option<u32>, Option<u32>)> {
    // Get duration
    let duration = probe_duration_seconds(path)?;

    // Get width
    let width_output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
        .ok()?;

    let width = String::from_utf8_lossy(&width_output.stdout).trim().parse::<u32>().ok();

    // Get height
    let height_output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=height",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
        .ok()?;

    let height = String::from_utf8_lossy(&height_output.stdout)
        .trim()
        .parse::<u32>()
        .ok();

    Some((duration, width, height))
}

/// Builds a yt-dlp format string optimized for Telegram compatibility.
///
/// Prioritizes H.264/AAC (avc1/mp4a) codecs to ensure Telegram can play videos correctly.
/// Falls back through lower resolutions if the requested quality is unavailable with
/// compatible codecs.
///
/// # Arguments
///
/// * `requested_height` - Optional preferred video height (e.g., 720, 1080)
///
/// # Returns
///
/// A yt-dlp format selection string
pub fn build_telegram_safe_format(requested_height: Option<u32>) -> String {
    // List of heights for sequential attempts (remove duplicates)
    let mut heights = vec![1080, 720, 480, 360, 240];
    if let Some(h) = requested_height {
        if !heights.contains(&h) {
            heights.insert(0, h);
        } else {
            // Move requested height to the front for priority
            heights.retain(|&v| v != h);
            heights.insert(0, h);
        }
    }

    let mut parts: Vec<String> = Vec::new();

    for h in heights {
        let filt = format!("[height<={h}]");
        // First, maximally compatible H.264 + AAC combinations
        parts.push(format!("bv*{filt}[vcodec^=avc1]+ba[acodec^=mp4a]"));
        // Alternative: explicit mp4/m4a tracks
        parts.push(format!("bv*{filt}[vcodec^=avc1][ext=mp4]+ba[ext=m4a]"));
    }

    // Fallbacks if no avc1/mp4a found
    parts.push("bestvideo[ext=mp4]+bestaudio[ext=m4a]".to_string());
    parts.push("best[ext=mp4]".to_string());
    parts.push("best".to_string());

    parts.join("/")
}

/// Finds the actual downloaded file path after yt-dlp download.
///
/// yt-dlp may add suffixes like (1).mp4, (2).mp4 if a file already exists.
///
/// # Arguments
///
/// * `expected_path` - The expected file path
///
/// # Returns
///
/// The actual file path found, or an error if not found
pub fn find_actual_downloaded_file(expected_path: &str) -> Result<String, AppError> {
    let path = Path::new(expected_path);

    // If file exists at expected path - return it
    if path.exists() {
        log::debug!("File found at expected path: {}", expected_path);
        return Ok(expected_path.to_string());
    }

    log::warn!("File not found at expected path: {}", expected_path);

    // Get directory and base file name
    let parent_dir = path
        .parent()
        .ok_or_else(|| AppError::Download(format!("Cannot get parent directory for: {}", expected_path)))?;

    let file_stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| AppError::Download(format!("Cannot get file stem for: {}", expected_path)))?;

    let file_extension = path.extension().and_then(|s| s.to_str()).unwrap_or("mp4");

    // Search for files starting with the base name
    let dir_entries =
        fs::read_dir(parent_dir).map_err(|e| AppError::Download(format!("Failed to read downloads dir: {}", e)))?;

    let mut found_files = Vec::new();
    for entry in dir_entries {
        if let Ok(entry) = entry {
            let file_name = entry.file_name();
            let file_name_str = file_name.to_string_lossy();

            // Check if filename starts with our base name and has the correct extension
            // yt-dlp may add suffixes like (1).mp4, (2).mp4 to the filename
            // file_stem already contains the timestamp, so check for exact match or prefix
            let matches_pattern =
                file_name_str.starts_with(file_stem) && file_name_str.ends_with(&format!(".{}", file_extension));

            if matches_pattern {
                let full_path = entry.path().to_string_lossy().to_string();
                found_files.push(full_path);
            }
        }
    }

    // If multiple files found, take the last one (most likely the newest)
    let actual_path = found_files
        .last()
        .ok_or_else(|| {
            log::error!("No matching files found in directory: {}", parent_dir.display());
            AppError::Download(format!(
                "Downloaded file not found at {} or in directory",
                expected_path
            ))
        })?
        .clone();
    log::info!(
        "Found actual downloaded file: {} (searched for: {})",
        actual_path,
        expected_path
    );

    Ok(actual_path)
}

/// Gets metadata from yt-dlp (faster than HTTP parsing).
///
/// Uses async command to avoid blocking the runtime.
/// Checks cache before making requests to yt-dlp.
///
/// # Arguments
///
/// * `admin_bot` - Optional bot for admin notifications on errors
/// * `user_chat_id` - Optional user chat ID for error context
/// * `url` - URL to fetch metadata for
///
/// # Returns
///
/// Tuple of (title, artist) if successful
pub async fn get_metadata_from_ytdlp(
    admin_bot: Option<&Bot>,
    user_chat_id: Option<ChatId>,
    url: &Url,
) -> Result<(String, String), AppError> {
    // Check cache, but ignore "Unknown Track" and "NA" in artist
    if let Some((title, artist)) = cache::get_cached_metadata(url).await {
        if title.trim() != "Unknown Track" && !title.trim().is_empty() {
            // If artist is empty or "NA" - ignore cache and get fresh data
            if artist.trim().is_empty() || artist.trim() == "NA" {
                log::debug!("Ignoring cached metadata with empty/NA artist for URL: {}", url);
            } else {
                log::debug!("Metadata cache hit for URL: {}", url);
                return Ok((title, artist));
            }
        } else {
            log::warn!("Ignoring invalid cached metadata '{}' for URL: {}", title, url);
        }
    }

    log::debug!("Metadata cache miss for URL: {}", url);
    let ytdl_bin = &*config::YTDL_BIN;
    log::debug!("Using downloader binary: {}", ytdl_bin);
    log::debug!("Fetching metadata for URL: {}", url);

    // Build arguments with cookies support
    // Use --print for more reliable metadata retrieval
    let mut args_vec: Vec<String> = vec![
        "--print".to_string(),
        "%(title)s".to_string(),
        "--no-playlist".to_string(),
        "--skip-download".to_string(),
    ];

    // Add cookies arguments
    let mut temp_args: Vec<&str> = vec![];
    add_cookies_args(&mut temp_args);
    for arg in temp_args {
        args_vec.push(arg.to_string());
    }

    // Do NOT use android client!
    // YouTube changed policy: now Android requires PO Token
    // Use default web client which works with cookies

    // Use Node.js for YouTube n-challenge solving
    args_vec.push("--js-runtimes".to_string());
    args_vec.push("node".to_string());

    args_vec.push("--no-check-certificate".to_string());
    args_vec.push(url.as_str().to_string());

    let args: Vec<&str> = args_vec.iter().map(|s| s.as_str()).collect();

    // Log full command for debugging
    let command_str = format!("{} {}", ytdl_bin, args.join(" "));
    log::info!("[DEBUG] yt-dlp command for metadata: {}", command_str);

    // Get title using async command with timeout
    let title_output = timeout(
        config::download::ytdlp_timeout(),
        TokioCommand::new(ytdl_bin).args(&args).output(),
    )
    .await
    .map_err(|_| {
        log::error!(
            "yt-dlp command timed out after {} seconds",
            config::download::YTDLP_TIMEOUT_SECS
        );
        metrics::record_error("download", "metadata_timeout");
        AppError::Download("yt-dlp command timed out".to_string())
    })?
    .map_err(|e| {
        log::error!("Failed to execute {}: {}", ytdl_bin, e);
        metrics::record_error("download", "metadata_spawn");
        AppError::Download(format!("Failed to get title: {}", e))
    })?;

    log::debug!(
        "yt-dlp exit status: {:?}, stdout length: {}",
        title_output.status,
        title_output.stdout.len()
    );

    if !title_output.status.success() {
        let stderr = String::from_utf8_lossy(&title_output.stderr);
        let error_type = analyze_ytdlp_error(&stderr);

        // Record error metric
        let error_category = match error_type {
            YtDlpErrorType::InvalidCookies => "invalid_cookies",
            YtDlpErrorType::BotDetection => "bot_detection",
            YtDlpErrorType::VideoUnavailable => "video_unavailable",
            YtDlpErrorType::NetworkError => "network",
            YtDlpErrorType::FragmentError => "fragment_error",
            YtDlpErrorType::Unknown => "ytdlp_unknown",
        };
        let operation = format!("metadata:{}", error_category);
        metrics::record_error("download", &operation);

        // Log detailed error information
        log::error!("yt-dlp failed to get metadata, error type: {:?}", error_type);
        log::error!("yt-dlp stderr: {}", stderr);

        // If admin notification needed - send details to Telegram admin
        if should_notify_admin(&error_type) {
            log::warn!("This error requires administrator attention!");
            if let Some(bot) = admin_bot {
                let mut text = String::new();
                text.push_str("YTDLP ERROR (metadata)\n");
                if let Some(chat_id) = user_chat_id {
                    text.push_str(&format!("user_chat_id: {}\n", chat_id.0));
                }
                text.push_str(&format!("url: {}\n", url));
                text.push_str(&format!("error_type: {:?}\n\n", error_type));
                text.push_str("command:\n");
                text.push_str(&command_str);
                text.push_str("\n\nstderr:\n");
                text.push_str(&stderr);
                notify_admin_text(bot, &text).await;
            }
        }

        // Return user-friendly error message
        return Err(AppError::Download(get_error_message(&error_type)));
    }

    let title = String::from_utf8_lossy(&title_output.stdout).trim().to_string();

    // Check that title is not empty
    if title.is_empty() {
        log::error!("yt-dlp returned empty title for URL: {}", url);
        metrics::record_error("download", "metadata_empty_title");
        return Err(AppError::Download(
            "Failed to get video title. Video might be unavailable or private.".to_string(),
        ));
    }

    log::info!("Successfully got metadata from yt-dlp: title='{}'", title);

    // Get artist via --print "%(artist)s"
    let mut artist_args_vec: Vec<String> = vec![
        "--print".to_string(),
        "%(artist)s".to_string(),
        "--no-playlist".to_string(),
        "--skip-download".to_string(),
    ];

    // Add cookies arguments
    let mut temp_args: Vec<&str> = vec![];
    add_cookies_args(&mut temp_args);
    for arg in temp_args {
        artist_args_vec.push(arg.to_string());
    }

    // Use Node.js for YouTube n-challenge solving
    artist_args_vec.push("--js-runtimes".to_string());
    artist_args_vec.push("node".to_string());

    artist_args_vec.push("--no-check-certificate".to_string());
    artist_args_vec.push(url.as_str().to_string());

    let artist_args: Vec<&str> = artist_args_vec.iter().map(|s| s.as_str()).collect();

    let artist_output = timeout(
        config::download::ytdlp_timeout(),
        TokioCommand::new(ytdl_bin).args(&artist_args).output(),
    )
    .await
    .ok(); // Not critical, ignore timeout errors

    let mut artist = artist_output
        .and_then(|result| result.ok())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default();

    // If artist is empty, "NA" or contains only whitespace - get channel/uploader
    if artist.trim().is_empty() || artist.trim() == "NA" {
        log::debug!("Artist is empty or 'NA', trying to get channel/uploader");

        // Try to get uploader (channel name)
        let mut uploader_args_vec: Vec<String> = vec![
            "--print".to_string(),
            "%(uploader)s".to_string(),
            "--no-playlist".to_string(),
            "--skip-download".to_string(),
        ];

        // Add cookies arguments
        let mut temp_args: Vec<&str> = vec![];
        add_cookies_args(&mut temp_args);
        for arg in temp_args {
            uploader_args_vec.push(arg.to_string());
        }

        // Use Node.js for YouTube n-challenge solving
        uploader_args_vec.push("--js-runtimes".to_string());
        uploader_args_vec.push("node".to_string());

        uploader_args_vec.push("--no-check-certificate".to_string());
        uploader_args_vec.push(url.as_str().to_string());

        let uploader_args: Vec<&str> = uploader_args_vec.iter().map(|s| s.as_str()).collect();

        let uploader_output = timeout(
            config::download::ytdlp_timeout(),
            TokioCommand::new(ytdl_bin).args(&uploader_args).output(),
        )
        .await
        .ok();

        let uploader = uploader_output
            .and_then(|result| result.ok())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .unwrap_or_default();

        if !uploader.trim().is_empty() && uploader.trim() != "NA" {
            artist = uploader;
            log::info!("Using uploader/channel as artist: '{}'", artist);
        } else {
            log::warn!("Could not get artist or uploader, leaving empty");
        }
    }

    // Cache only if title is not empty and not "Unknown Track"
    if !title.trim().is_empty() && title.trim() != "Unknown Track" {
        cache::cache_metadata(url, title.clone(), artist.clone()).await;
    } else {
        log::warn!("Not caching metadata with invalid title: '{}'", title);
    }

    log::info!("Got metadata from yt-dlp: title='{}', artist='{}'", title, artist);
    Ok((title, artist))
}
