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
use crate::download::error::DownloadError;
use crate::download::ytdlp_errors::{analyze_ytdlp_error, get_error_message, should_notify_admin, YtDlpErrorType};
use crate::storage::cache;
use crate::telegram::notifications::notify_admin_text;
use crate::telegram::Bot;
use once_cell::sync::Lazy;
use std::fs;
use std::path::Path;
use std::process::Command;
use teloxide::prelude::*;
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;
use url::Url;

// =============================================================================
// Cached static strings for yt-dlp arguments (prevents memory leaks)
// =============================================================================

/// Cached resolved cookies file path (computed once at first use)
static CACHED_COOKIES_PATH: Lazy<Option<String>> = Lazy::new(|| {
    if let Some(ref cookies_file) = *config::YTDL_COOKIES_FILE {
        if cookies_file.is_empty() {
            return None;
        }

        // Convert relative path to absolute
        let cookies_path = if std::path::Path::new(cookies_file).is_absolute() {
            cookies_file.clone()
        } else {
            shellexpand::tilde(cookies_file).to_string()
        };

        let cookies_path_buf = std::path::Path::new(&cookies_path);
        if cookies_path_buf.exists() {
            // Get absolute path
            cookies_path_buf
                .canonicalize()
                .ok()
                .map(|p| p.to_string_lossy().to_string())
        } else {
            log::error!("Cookies file not found at startup: {}", cookies_path);
            None
        }
    } else {
        None
    }
});

/// Returns cached cookies path as &'static str (no allocation per call)
fn get_cached_cookies_path() -> Option<&'static str> {
    CACHED_COOKIES_PATH.as_ref().map(|s| s.as_str())
}

/// Cached resolved Instagram cookies file path (computed once at first use)
static CACHED_INSTAGRAM_COOKIES_PATH: Lazy<Option<String>> = Lazy::new(|| {
    if let Some(ref cookies_file) = *config::INSTAGRAM_COOKIES_FILE {
        if cookies_file.is_empty() {
            return None;
        }

        let cookies_path = if std::path::Path::new(cookies_file).is_absolute() {
            cookies_file.clone()
        } else {
            shellexpand::tilde(cookies_file).to_string()
        };

        let cookies_path_buf = std::path::Path::new(&cookies_path);
        if cookies_path_buf.exists() {
            cookies_path_buf
                .canonicalize()
                .ok()
                .map(|p| p.to_string_lossy().to_string())
        } else {
            log::warn!("Instagram cookies file not found at startup: {}", cookies_path);
            None
        }
    } else {
        None
    }
});

/// Returns cached Instagram cookies path as &'static str (no allocation per call)
fn get_cached_instagram_cookies_path() -> Option<&'static str> {
    CACHED_INSTAGRAM_COOKIES_PATH.as_ref().map(|s| s.as_str())
}

/// Cached WARP proxy URL (from config, computed once)
static CACHED_WARP_PROXY: Lazy<Option<String>> = Lazy::new(|| {
    config::proxy::WARP_PROXY
        .as_ref()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
});

/// Returns cached WARP proxy URL as &'static str (no allocation per call)
fn get_cached_warp_proxy() -> Option<&'static str> {
    CACHED_WARP_PROXY.as_ref().map(|s| s.as_str())
}

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
    /// Human-readable description for logs (e.g., "WARP")
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

/// Returns ordered list of proxies to try: WARP (primary) → None (direct)
///
/// This enables automatic failover when a proxy fails:
/// 1. First try WARP proxy (free Cloudflare IP)
/// 2. If WARP fails, try direct connection (no proxy)
pub fn get_proxy_chain() -> Vec<Option<ProxyConfig>> {
    let mut chain = Vec::new();

    // Primary: Proxy from WARP_PROXY environment variable
    if let Some(ref warp_proxy) = *config::proxy::WARP_PROXY {
        let proxy_url = warp_proxy.trim();
        // Ignore "none", "disabled", or empty values
        if !proxy_url.is_empty() && proxy_url != "none" && proxy_url != "disabled" {
            // Auto-detect proxy name from URL
            let proxy_name = if proxy_url.contains("geonode.com") {
                "Geonode Residential"
            } else if proxy_url.contains("89.124.69.143") || proxy_url.contains("cloudflare") {
                "WARP (Cloudflare)"
            } else {
                "Custom Proxy"
            };

            log::info!("🌐 Using proxy: {} ({})", proxy_name, proxy_url);

            chain.push(Some(ProxyConfig::new(proxy_url.to_string(), proxy_name)));
        }
    }

    // Last resort: No proxy (direct connection)
    chain.push(None);

    log::info!("🔗 Proxy chain configured: {} proxy(ies)", chain.len());

    chain
}

/// Checks if an error is proxy-related and should trigger fallback to next proxy
pub fn is_proxy_related_error(error_msg: &str) -> bool {
    let error_type = analyze_ytdlp_error(error_msg);
    if matches!(error_type, YtDlpErrorType::InvalidCookies) {
        return false;
    }

    if matches!(error_type, YtDlpErrorType::BotDetection | YtDlpErrorType::NetworkError) {
        return true;
    }

    let error_lower = error_msg.to_lowercase();
    error_lower.contains("proxy")
        || error_lower.contains("proxy authentication")
        || error_lower.contains("unable to connect to proxy")
        || error_lower.contains("tunnel")
        || error_lower.contains("socks")
        || error_lower.contains("407")
        || error_lower.contains("forbidden")
        || error_lower.contains("403")
        || error_lower.contains("timed out")
        || error_lower.contains("timeout")
        || error_lower.contains("dns")
        || error_lower.contains("connection refused")
        || error_lower.contains("connection reset")
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
/// This is a convenience wrapper that uses the default proxy chain (WARP → direct).
/// For retry logic with specific proxy, use `add_cookies_args_with_proxy` instead.
pub fn add_cookies_args(args: &mut Vec<&str>) {
    // Use first available proxy from the chain (WARP or direct)
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
/// Uses cached static strings to avoid memory leaks (previously used Box::leak).
pub fn add_cookies_args_with_proxy(args: &mut Vec<&str>, proxy: Option<&ProxyConfig>) {
    // Add proxy if provided - use cached WARP proxy URL if it matches
    if let Some(proxy_config) = proxy {
        log::info!("Using proxy [{}]: {}", proxy_config.name, proxy_config.masked_url());
        args.push("--proxy");

        // Check if this is the WARP proxy (use cached static string)
        if let Some(cached_warp) = get_cached_warp_proxy() {
            if proxy_config.url.trim() == cached_warp {
                args.push(cached_warp);
            } else {
                // Different proxy - this shouldn't happen with current config, but handle it
                // Use the cached warp anyway since it's the only proxy in config
                log::warn!("Unexpected proxy URL, using cached WARP proxy");
                args.push(cached_warp);
            }
        } else {
            // No cached proxy - skip (shouldn't add --proxy without URL)
            log::warn!("Proxy requested but no cached proxy URL available");
        }
    } else {
        log::info!("No proxy configured, using direct connection");
    }

    // Add PO Token provider configuration for YouTube
    // bgutil HTTP server runs on port 4416 by default
    args.push("--extractor-args");
    args.push("youtubepot-bgutilhttp:base_url=http://127.0.0.1:4416");

    // Priority 1: Cookies file (use cached path - no allocation!)
    if let Some(cached_path) = get_cached_cookies_path() {
        // Validate format on first use (already logged at cache creation)
        args.push("--cookies");
        args.push(cached_path);
        log::debug!("Using cached cookies path: {}", cached_path);
        return;
    }

    // Fallback: check if file exists but wasn't cached (e.g., created after startup)
    if let Some(ref cookies_file) = *config::YTDL_COOKIES_FILE {
        if !cookies_file.is_empty() {
            let cookies_path = if std::path::Path::new(cookies_file).is_absolute() {
                cookies_file.clone()
            } else {
                shellexpand::tilde(cookies_file).to_string()
            };

            let cookies_path_buf = std::path::Path::new(&cookies_path);
            if !cookies_path_buf.exists() {
                log::error!("Cookies file not found: {} (checked: {})", cookies_file, cookies_path);
                log::error!("   YouTube downloads will FAIL without valid cookies!");
                return;
            }
            // File exists but wasn't cached - this means it was created after startup
            // Log warning but don't use it (would require leak or owned strings)
            log::warn!(
                "Cookies file found but not cached (created after startup?): {}",
                cookies_path
            );
            log::warn!("Restart the bot to use the new cookies file");
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

/// Adds NO cookies and NO PO Token arguments (v5.0 modern yt-dlp mode).
///
/// This is the PRIMARY mode for yt-dlp 2026.02.04+ which automatically uses
/// `android_vr` + `web_safari` clients that don't require cookies or PO tokens.
/// This mode works for the vast majority of public videos without any auth.
///
/// # Arguments
///
/// * `args` - Vector of arguments for yt-dlp to modify
/// * `proxy` - Optional proxy configuration
pub fn add_no_cookies_args(args: &mut Vec<&str>, proxy: Option<&ProxyConfig>) {
    // Add proxy if provided - use cached WARP proxy URL
    if let Some(proxy_config) = proxy {
        log::info!(
            "[NO_COOKIES] Using proxy [{}]: {}",
            proxy_config.name,
            proxy_config.masked_url()
        );
        args.push("--proxy");
        // Use cached static string to avoid memory leak
        if let Some(cached_warp) = get_cached_warp_proxy() {
            args.push(cached_warp);
        } else {
            log::warn!("[NO_COOKIES] Proxy requested but no cached proxy URL");
        }
    } else {
        log::info!("[NO_COOKIES] No proxy, using direct connection");
    }

    // NO PO Token, NO cookies - let yt-dlp 2026+ use its default client selection
    // which automatically chooses android_vr + web_safari clients
    log::info!("[NO_COOKIES] Running WITHOUT cookies and WITHOUT PO Token (modern yt-dlp mode)");
}

/// Adds proxy and Instagram cookies arguments to yt-dlp command.
///
/// Similar to `add_cookies_args_with_proxy` but uses Instagram cookies file
/// instead of YouTube cookies and does NOT add PO Token / YouTube extractor-args.
///
/// # Arguments
///
/// * `args` - Vector of arguments for yt-dlp to modify
/// * `proxy` - Optional proxy configuration
///
/// # Returns
///
/// `true` if IG cookies were added, `false` if no IG cookies available
pub fn add_instagram_cookies_args_with_proxy(args: &mut Vec<&str>, proxy: Option<&ProxyConfig>) -> bool {
    // Add proxy if provided
    if let Some(proxy_config) = proxy {
        log::info!(
            "[IG_COOKIES] Using proxy [{}]: {}",
            proxy_config.name,
            proxy_config.masked_url()
        );
        args.push("--proxy");
        if let Some(cached_warp) = get_cached_warp_proxy() {
            args.push(cached_warp);
        } else {
            log::warn!("[IG_COOKIES] Proxy requested but no cached proxy URL");
        }
    }

    // Add Instagram cookies file
    if let Some(cached_path) = get_cached_instagram_cookies_path() {
        args.push("--cookies");
        args.push(cached_path);
        log::info!("[IG_COOKIES] Using Instagram cookies: {}", cached_path);
        return true;
    }

    // Fallback: check if file exists but wasn't cached (created after startup)
    if let Some(ref cookies_file) = *config::INSTAGRAM_COOKIES_FILE {
        if !cookies_file.is_empty() {
            let cookies_path = if std::path::Path::new(cookies_file).is_absolute() {
                cookies_file.clone()
            } else {
                shellexpand::tilde(cookies_file).to_string()
            };
            if std::path::Path::new(&cookies_path).exists() {
                log::warn!(
                    "[IG_COOKIES] Instagram cookies file found but not cached (created after startup?): {}",
                    cookies_path
                );
                log::warn!("Restart the bot to use the new Instagram cookies file");
            } else {
                log::debug!("[IG_COOKIES] Instagram cookies file not found: {}", cookies_path);
            }
        }
    }

    log::debug!("[IG_COOKIES] No Instagram cookies available");
    false
}

/// Adds ONLY PO Token arguments WITHOUT cookies (v4.0 fallback mode).
///
/// This is used when cookies fail but we want to try downloading anyway.
/// PO Token alone works for ~80% of public videos without authentication.
///
/// # Arguments
///
/// * `args` - Vector of arguments for yt-dlp to modify
/// * `proxy` - Optional proxy configuration
pub fn add_po_token_only_args(args: &mut Vec<&str>, proxy: Option<&ProxyConfig>) {
    // Add proxy if provided - use cached WARP proxy URL
    if let Some(proxy_config) = proxy {
        log::info!(
            "[PO_TOKEN_ONLY] Using proxy [{}]: {}",
            proxy_config.name,
            proxy_config.masked_url()
        );
        args.push("--proxy");
        // Use cached static string to avoid memory leak
        if let Some(cached_warp) = get_cached_warp_proxy() {
            args.push(cached_warp);
        } else {
            log::warn!("[PO_TOKEN_ONLY] Proxy requested but no cached proxy URL");
        }
    } else {
        log::info!("[PO_TOKEN_ONLY] No proxy, using direct connection");
    }

    // Add PO Token provider configuration for YouTube
    // This is the key - PO Token works WITHOUT cookies for most public videos
    args.push("--extractor-args");
    args.push("youtubepot-bgutilhttp:base_url=http://127.0.0.1:4416");

    log::info!("[PO_TOKEN_ONLY] Running WITHOUT cookies, using PO Token only");
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
        .map_err(|e| AppError::Download(DownloadError::Other(format!("Failed to check video stream: {}", e))))?;

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
        .map_err(|e| AppError::Download(DownloadError::Other(format!("Failed to check audio stream: {}", e))))?;

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
    let parent_dir = path.parent().ok_or_else(|| {
        AppError::Download(DownloadError::FileNotFound(format!(
            "Cannot get parent directory for: {}",
            expected_path
        )))
    })?;

    let file_stem = path.file_stem().and_then(|s| s.to_str()).ok_or_else(|| {
        AppError::Download(DownloadError::FileNotFound(format!(
            "Cannot get file stem for: {}",
            expected_path
        )))
    })?;

    let file_extension = path.extension().and_then(|s| s.to_str()).unwrap_or("mp4");

    // Search for files starting with the base name
    let dir_entries = fs::read_dir(parent_dir).map_err(|e| {
        AppError::Download(DownloadError::FileNotFound(format!(
            "Failed to read downloads dir: {}",
            e
        )))
    })?;

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
            AppError::Download(DownloadError::FileNotFound(format!(
                "Downloaded file not found at {} or in directory",
                expected_path
            )))
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

    // Use android_vr + web_safari clients (minimal bot detection, no PO token required)
    // android_vr works without authentication, web_safari as fallback
    args_vec.push("--extractor-args".to_string());
    args_vec.push("youtube:player_client=android_vr,web_safari;formats=missing_pot".to_string());

    // Use Deno for YouTube n-challenge solving (yt-dlp 2026+)
    args_vec.push("--js-runtimes".to_string());
    args_vec.push("deno".to_string());

    // Note: impersonate removed - not needed for android_vr client

    args_vec.push("--no-check-certificate".to_string());
    args_vec.push(url.as_str().to_string());

    let args: Vec<&str> = args_vec.iter().map(|s| s.as_str()).collect();

    // Log full command for debugging
    let command_str = format!("{} {}", ytdl_bin, args.join(" "));
    log::debug!("yt-dlp command for metadata: {}", command_str);

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
        AppError::Download(DownloadError::YtDlp("yt-dlp command timed out".to_string()))
    })?
    .map_err(|e| {
        log::error!("Failed to execute {}: {}", ytdl_bin, e);
        metrics::record_error("download", "metadata_spawn");
        AppError::Download(DownloadError::YtDlp(format!("Failed to get title: {}", e)))
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
            YtDlpErrorType::PostprocessingError => "postprocessing_error",
            YtDlpErrorType::DiskSpaceError => "disk_space_error",
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
        return Err(AppError::Download(DownloadError::YtDlp(get_error_message(&error_type))));
    }

    let title = String::from_utf8_lossy(&title_output.stdout).trim().to_string();

    // Check that title is not empty
    if title.is_empty() {
        log::error!("yt-dlp returned empty title for URL: {}", url);
        metrics::record_error("download", "metadata_empty_title");
        return Err(AppError::Download(DownloadError::YtDlp(
            "Failed to get video title. Video might be unavailable or private.".to_string(),
        )));
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

    artist_args_vec.push("--extractor-args".to_string());
    artist_args_vec.push("youtube:player_client=android_vr,web_safari;formats=missing_pot".to_string());
    artist_args_vec.push("--js-runtimes".to_string());
    artist_args_vec.push("deno".to_string());

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

        uploader_args_vec.push("--extractor-args".to_string());
        uploader_args_vec.push("youtube:player_client=android_vr,web_safari;formats=missing_pot".to_string());
        uploader_args_vec.push("--js-runtimes".to_string());
        uploader_args_vec.push("deno".to_string());

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

/// Get estimated file size for a URL before downloading
///
/// Returns the estimated file size in bytes, or None if not available.
/// This is used to reject downloads that would exceed size limits.
pub async fn get_estimated_filesize(url: &Url) -> Option<u64> {
    let ytdl_bin = &*config::YTDL_BIN;

    let mut args_vec: Vec<String> = vec![
        "--print".to_string(),
        "%(filesize_approx)s".to_string(),
        "--no-playlist".to_string(),
        "--skip-download".to_string(),
    ];

    // Add cookies arguments
    let mut temp_args: Vec<&str> = vec![];
    add_cookies_args(&mut temp_args);
    for arg in temp_args {
        args_vec.push(arg.to_string());
    }

    args_vec.push("--no-check-certificate".to_string());
    args_vec.push(url.as_str().to_string());

    let args: Vec<&str> = args_vec.iter().map(|s| s.as_str()).collect();

    log::debug!("Getting estimated filesize for URL: {}", url);

    // Run with short timeout since this is just a check
    let output = timeout(
        std::time::Duration::from_secs(30),
        TokioCommand::new(ytdl_bin).args(&args).output(),
    )
    .await;

    match output {
        Ok(Ok(result)) if result.status.success() => {
            let size_str = String::from_utf8_lossy(&result.stdout).trim().to_string();
            // yt-dlp returns "NA" if size is not available
            if size_str == "NA" || size_str.is_empty() {
                log::debug!("File size not available for URL: {}", url);
                return None;
            }
            match size_str.parse::<u64>() {
                Ok(size) => {
                    // Add 15% overhead for conversion/encoding overhead
                    let size_with_overhead = (size as f64 * 1.15) as u64;
                    log::info!(
                        "📦 Estimated file size for {}: {:.2} MB (with 15% overhead: {:.2} MB)",
                        url,
                        size as f64 / (1024.0 * 1024.0),
                        size_with_overhead as f64 / (1024.0 * 1024.0)
                    );
                    Some(size_with_overhead)
                }
                Err(_) => {
                    log::debug!("Failed to parse file size '{}' for URL: {}", size_str, url);
                    None
                }
            }
        }
        Ok(Ok(_)) | Ok(Err(_)) | Err(_) => {
            log::debug!("Could not get estimated filesize for: {}", url);
            None
        }
    }
}

/// Check if a URL is a livestream
///
/// Returns `true` if the video is a live stream, `false` otherwise.
/// This is used to reject livestream URLs before starting downloads.
pub async fn is_livestream(url: &Url) -> bool {
    let ytdl_bin = &*config::YTDL_BIN;

    let mut args_vec: Vec<String> = vec![
        "--print".to_string(),
        "%(is_live)s".to_string(),
        "--no-playlist".to_string(),
        "--skip-download".to_string(),
    ];

    // Add cookies arguments
    let mut temp_args: Vec<&str> = vec![];
    add_cookies_args(&mut temp_args);
    for arg in temp_args {
        args_vec.push(arg.to_string());
    }

    args_vec.push("--no-check-certificate".to_string());
    args_vec.push(url.as_str().to_string());

    let args: Vec<&str> = args_vec.iter().map(|s| s.as_str()).collect();

    log::debug!("Checking if URL is livestream: {}", url);

    // Run with short timeout since this is just a check
    let output = timeout(
        std::time::Duration::from_secs(30),
        TokioCommand::new(ytdl_bin).args(&args).output(),
    )
    .await;

    match output {
        Ok(Ok(result)) if result.status.success() => {
            let is_live_str = String::from_utf8_lossy(&result.stdout).trim().to_lowercase();
            let is_live = is_live_str == "true" || is_live_str == "1";
            if is_live {
                log::warn!("🔴 URL is a LIVE STREAM, will be rejected: {}", url);
            } else {
                log::debug!("URL is not a livestream (is_live={})", is_live_str);
            }
            is_live
        }
        Ok(Ok(result)) => {
            // Command failed, check stderr for clues
            let stderr = String::from_utf8_lossy(&result.stderr);
            if stderr.contains("live") || stderr.contains("is live") {
                log::warn!("🔴 URL appears to be a livestream based on error: {}", url);
                true
            } else {
                log::debug!("Livestream check failed for {}: {}", url, stderr);
                false
            }
        }
        Ok(Err(e)) => {
            log::debug!("Failed to check if URL is livestream: {}", e);
            false
        }
        Err(_) => {
            log::debug!("Livestream check timed out for: {}", url);
            false
        }
    }
}
