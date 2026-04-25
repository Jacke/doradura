//! Metadata extraction, cookie management, proxy configuration, and media probing.
//! No Telegram dependencies — errors are logged instead of sent to admin.

use crate::core::config;
use crate::core::error::AppError;
use crate::core::metrics;
use crate::download::error::DownloadError;
use crate::download::ytdlp_errors::{YtDlpErrorType, analyze_ytdlp_error, get_error_message, should_notify_admin};
use crate::storage::cache;
use fs_err as fs;
use std::path::Path;
use std::sync::LazyLock;
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;
use url::Url;

/// Extract only the first line from yt-dlp stdout.
///
/// yt-dlp `--print` on playlist/set URLs outputs one line per track.
/// Taking all stdout would concatenate every track's metadata into one string.
fn first_line_of_stdout(stdout: &[u8]) -> String {
    String::from_utf8_lossy(stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

// =============================================================================
// Cached static strings for yt-dlp arguments (prevents memory leaks)
// =============================================================================

/// Cached resolved cookies file path (computed once at first use)
static CACHED_COOKIES_PATH: LazyLock<Option<String>> = LazyLock::new(|| {
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

/// Returns cached cookies path as `&'static str` (no allocation per call)
fn get_cached_cookies_path() -> Option<&'static str> {
    CACHED_COOKIES_PATH.as_ref().map(|s| s.as_str())
}

/// Cached resolved Instagram cookies file path (computed once at first use)
static CACHED_INSTAGRAM_COOKIES_PATH: LazyLock<Option<String>> = LazyLock::new(|| {
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

/// Returns cached Instagram cookies path as `&'static str` (no allocation per call)
fn get_cached_instagram_cookies_path() -> Option<&'static str> {
    CACHED_INSTAGRAM_COOKIES_PATH.as_ref().map(|s| s.as_str())
}

/// Cached WARP proxy URL (from config, computed once)
static CACHED_WARP_PROXY: LazyLock<Option<String>> = LazyLock::new(|| {
    config::proxy::WARP_PROXY
        .as_ref()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
});

/// Returns cached WARP proxy URL as `&'static str` (no allocation per call)
fn get_cached_warp_proxy() -> Option<&'static str> {
    CACHED_WARP_PROXY.as_ref().map(|s| s.as_str())
}

/// Masks the password component in a proxy URL for safe logging.
///
/// Transforms `"http://user:secret@host:port"` to `"http://user:***@host:port"`.
pub fn mask_proxy_password(proxy_url: &str) -> String {
    if let Some(at_pos) = proxy_url.rfind('@')
        && let Some(colon_pos) = proxy_url[..at_pos].rfind(':')
    {
        let prefix = &proxy_url[..colon_pos + 1];
        let suffix = &proxy_url[at_pos..];
        return format!("{}***{}", prefix, suffix);
    }
    proxy_url.to_string()
}

/// Represents a proxy configuration with URL and description for logging.
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    /// Proxy URL (e.g., `"socks5://host:port"` or `"http://user:pass@host:port"`)
    pub url: String,
    /// Human-readable description for logs (e.g., `"WARP"`)
    pub name: String,
}

impl ProxyConfig {
    pub fn new(url: String, name: &str) -> Self {
        Self {
            url,
            name: name.to_string(),
        }
    }

    /// Returns the masked URL suitable for logging (password redacted).
    pub fn masked_url(&self) -> String {
        mask_proxy_password(&self.url)
    }
}

/// Returns the ordered list of proxies to try: WARP (primary) → direct connection.
///
/// This enables automatic failover when a proxy fails:
/// 1. First try WARP proxy (Cloudflare IP)
/// 2. If WARP fails, try direct connection (no proxy)
pub fn get_proxy_chain() -> Vec<Option<ProxyConfig>> {
    let mut chain = Vec::new();

    if let Some(ref warp_proxy) = *config::proxy::WARP_PROXY {
        let proxy_url = warp_proxy.trim();
        if !proxy_url.is_empty() && proxy_url != "none" && proxy_url != "disabled" {
            let proxy_name = if proxy_url.contains("geonode.com") {
                "Geonode Residential"
            } else if proxy_url.contains("89.124.69.143") || proxy_url.contains("cloudflare") {
                "WARP (Cloudflare)"
            } else {
                "Custom Proxy"
            };

            log::debug!("Using proxy: {} ({})", proxy_name, proxy_url);
            chain.push(Some(ProxyConfig::new(proxy_url.to_string(), proxy_name)));
        }
    }

    // Last resort: no proxy (direct connection)
    chain.push(None);

    log::debug!("Proxy chain configured: {} proxy(ies)", chain.len());
    chain
}

/// Returns `true` if the error message indicates a proxy-related failure.
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

/// Validates that a cookies file uses the Netscape HTTP Cookie File format.
///
/// The Netscape format starts with `"# Netscape HTTP Cookie File"` or
/// `"# HTTP Cookie File"` and contains lines with seven tab-separated fields.
///
/// Returns `true` if the file exists and has valid Netscape format, `false` otherwise.
pub fn validate_cookies_file_format(cookies_file: &str) -> bool {
    if let Ok(contents) = fs_err::read_to_string(cookies_file) {
        let has_header = contents.lines().any(|line| {
            line.trim().starts_with("# Netscape HTTP Cookie File") || line.trim().starts_with("# HTTP Cookie File")
        });

        let has_cookies = contents.lines().any(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#') && trimmed.split('\t').count() >= 7
        });

        has_header && has_cookies
    } else {
        false
    }
}

/// Adds proxy, cookie, and PO Token arguments to a yt-dlp argument vector.
///
/// This is a convenience wrapper that uses the default proxy chain (WARP → direct).
/// For retry logic with a specific proxy, use [`add_cookies_args_with_proxy`] instead.
pub fn add_cookies_args(args: &mut Vec<&str>) {
    let proxy_chain = get_proxy_chain();
    let first_proxy = proxy_chain.into_iter().find(|p| p.is_some()).flatten();
    add_cookies_args_with_proxy(args, first_proxy.as_ref(), None);
}

// Experimental features graduated to main workflow

/// Returns POT argument for `add_cookies_args_with_proxy`:
/// Always returns `Some("")` (skip bgutil) — graduated from experimental.
pub fn default_pot_token() -> Option<&'static str> {
    Some("")
}

/// Returns YouTube extractor-args with `formats=dashy` (enables -N parallelism).
/// Graduated from experimental to default.
pub fn default_youtube_extractor_args() -> &'static str {
    "youtube:player_client=default;formats=dashy"
}

/// Adds proxy, cookie, and PO Token arguments with a specific proxy configuration.
///
/// # Arguments
///
/// * `args` — Vector of yt-dlp arguments to extend
/// * `proxy` — Optional proxy configuration; `None` means direct connection
pub fn add_cookies_args_with_proxy<'a>(
    args: &mut Vec<&'a str>,
    proxy: Option<&ProxyConfig>,
    cached_pot_arg: Option<&'a str>,
) {
    if let Some(proxy_config) = proxy {
        log::info!("Using proxy [{}]: {}", proxy_config.name, proxy_config.masked_url());
        args.push("--proxy");

        if let Some(cached_warp) = get_cached_warp_proxy() {
            if proxy_config.url.trim() == cached_warp {
                args.push(cached_warp);
            } else {
                log::warn!("Unexpected proxy URL, using cached WARP proxy");
                args.push(cached_warp);
            }
        } else {
            log::warn!("Proxy requested but no cached proxy URL available");
        }
    } else {
        log::info!("No proxy configured, using direct connection");
    }

    // PO Token provider: None = bgutil plugin, Some("") = skip entirely (experimental)
    // Some(token) = use cached token
    match cached_pot_arg {
        Some("") => {
            // Experimental: skip bgutil POT provider — cookies are sufficient for YouTube
        }
        Some(pot_arg) => {
            args.push("--extractor-args");
            args.push(pot_arg);
        }
        None => {
            args.push("--extractor-args");
            args.push("youtubepot-bgutilhttp:base_url=http://127.0.0.1:4416");
        }
    }

    // Priority 1: Cookies file (use cached path — no allocation)
    if let Some(cached_path) = get_cached_cookies_path() {
        args.push("--cookies");
        args.push(cached_path);
        log::debug!("Using cached cookies path: {}", cached_path);
        return;
    }

    // Fallback: check if file exists but wasn't cached (created after startup)
    if let Some(ref cookies_file) = *config::YTDL_COOKIES_FILE
        && !cookies_file.is_empty()
    {
        let cookies_path = if std::path::Path::new(cookies_file).is_absolute() {
            cookies_file.clone()
        } else {
            shellexpand::tilde(cookies_file).to_string()
        };

        let cookies_path_buf = std::path::Path::new(&cookies_path);
        if !cookies_path_buf.exists() {
            log::error!("Cookies file not found: {} (checked: {})", cookies_file, cookies_path);
            log::error!("YouTube downloads will FAIL without valid cookies!");
            return;
        }
        log::warn!(
            "Cookies file found but not cached (created after startup?): {}",
            cookies_path
        );
        log::warn!("Restart the bot to use the new cookies file");
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

        #[cfg(target_os = "macos")]
        {
            log::warn!("macOS USERS:");
            log::warn!("   Browser cookie extraction requires Full Disk Access.");
            log::warn!("   It's MUCH EASIER to export cookies to a file!");
            log::warn!("   See: MACOS_COOKIES_FIX.md for step-by-step guide");
            log::warn!("   Quick fix:");
            log::warn!("   1. Install Chrome extension: Get cookies.txt LOCALLY");
            log::warn!("   2. Go to youtube.com -> login");
            log::warn!("   3. Click extension -> Export -> save as youtube_cookies.txt");
            log::warn!("   4. Set YTDL_COOKIES_FILE=youtube_cookies.txt and restart");
        }

        #[cfg(not(target_os = "macos"))]
        {
            log::warn!("AUTOMATIC COOKIE EXTRACTION (Recommended):");
            log::warn!("   1. Login to YouTube in your browser (chrome/firefox/etc)");
            log::warn!("   2. Install dependencies: pip3 install keyring pycryptodomex");
            log::warn!("   3. Set browser: export YTDL_COOKIES_BROWSER=chrome");
            log::warn!("      Supported: chrome, firefox, safari, brave, chromium, edge, opera, vivaldi");
            log::warn!("   4. Restart the bot");
            log::warn!("OR EXPORT TO FILE (Alternative):");
            log::warn!("   1. Export cookies from browser to youtube_cookies.txt");
            log::warn!("   2. Set: export YTDL_COOKIES_FILE=youtube_cookies.txt");
            log::warn!("   3. Restart the bot");
        }

        log::warn!("-----------------------------------------------------------");
    }
}

/// Adds no cookies and no PO Token arguments (v5.0 modern yt-dlp mode).
///
/// This is the primary mode for yt-dlp 2026.02.04+, which automatically uses
/// `android_vr` + `web_safari` clients that don't require cookies or PO tokens.
///
/// # Arguments
///
/// * `args` — Vector of yt-dlp arguments to extend
/// * `proxy` — Optional proxy configuration
pub fn add_no_cookies_args(args: &mut Vec<&str>, proxy: Option<&ProxyConfig>) {
    if let Some(proxy_config) = proxy {
        log::info!(
            "[NO_COOKIES] Using proxy [{}]: {}",
            proxy_config.name,
            proxy_config.masked_url()
        );
        args.push("--proxy");
        if let Some(cached_warp) = get_cached_warp_proxy() {
            args.push(cached_warp);
        } else {
            log::warn!("[NO_COOKIES] Proxy requested but no cached proxy URL");
        }
    } else {
        log::info!("[NO_COOKIES] No proxy, using direct connection");
    }

    log::info!("[NO_COOKIES] Running WITHOUT cookies and WITHOUT PO Token (modern yt-dlp mode)");
}

/// Adds proxy and Instagram cookies arguments to a yt-dlp argument vector.
///
/// Similar to [`add_cookies_args_with_proxy`] but uses the Instagram cookies file
/// and does NOT add PO Token / YouTube extractor-args.
///
/// Returns `true` if Instagram cookies were added, `false` if none are available.
pub fn add_instagram_cookies_args_with_proxy(args: &mut Vec<&str>, proxy: Option<&ProxyConfig>) -> bool {
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

    if let Some(cached_path) = get_cached_instagram_cookies_path() {
        args.push("--cookies");
        args.push(cached_path);
        log::info!("[IG_COOKIES] Using Instagram cookies: {}", cached_path);
        return true;
    }

    if let Some(ref cookies_file) = *config::INSTAGRAM_COOKIES_FILE
        && !cookies_file.is_empty()
    {
        let cookies_path = if std::path::Path::new(cookies_file).is_absolute() {
            cookies_file.clone()
        } else {
            shellexpand::tilde(cookies_file).to_string()
        };
        if std::path::Path::new(&cookies_path).exists() {
            log::warn!(
                "[IG_COOKIES] Instagram cookies found but not cached (created after startup?): {}",
                cookies_path
            );
            log::warn!("Restart the bot to use the new Instagram cookies file");
        } else {
            log::debug!("[IG_COOKIES] Instagram cookies file not found: {}", cookies_path);
        }
    }

    log::debug!("[IG_COOKIES] No Instagram cookies available");
    false
}

/// Adds ONLY PO Token arguments WITHOUT cookies (v4.0 fallback mode).
///
/// This is used when cookies fail but PO Token alone is tried.
/// PO Token alone works for approximately 80% of public videos without authentication.
///
/// # Arguments
///
/// * `args` — Vector of yt-dlp arguments to extend
/// * `proxy` — Optional proxy configuration
pub fn add_po_token_only_args(args: &mut Vec<&str>, proxy: Option<&ProxyConfig>) {
    if let Some(proxy_config) = proxy {
        log::info!(
            "[PO_TOKEN_ONLY] Using proxy [{}]: {}",
            proxy_config.name,
            proxy_config.masked_url()
        );
        args.push("--proxy");
        if let Some(cached_warp) = get_cached_warp_proxy() {
            args.push(cached_warp);
        } else {
            log::warn!("[PO_TOKEN_ONLY] Proxy requested but no cached proxy URL");
        }
    } else {
        log::info!("[PO_TOKEN_ONLY] No proxy, using direct connection");
    }

    args.push("--extractor-args");
    args.push("youtubepot-bgutilhttp:base_url=http://127.0.0.1:4416");

    log::info!("[PO_TOKEN_ONLY] Running WITHOUT cookies, using PO Token only");
}

/// Probes media file duration using `ffprobe`.
///
/// Uses `tokio::process::Command` with a 30-second timeout to avoid blocking
/// the async runtime.
///
/// # Returns
///
/// Duration in seconds if successful, `None` otherwise.
pub async fn probe_duration_seconds(path: &str) -> Option<u32> {
    use crate::core::process::{FFPROBE_TIMEOUT, run_with_timeout};

    let mut cmd = TokioCommand::new("ffprobe");
    cmd.args([
        "-v",
        "error",
        "-show_entries",
        "format=duration",
        "-of",
        "default=noprint_wrappers=1:nokey=1",
        path,
    ]);
    let output = run_with_timeout(&mut cmd, FFPROBE_TIMEOUT).await.ok()?;

    let duration_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if duration_str.is_empty() {
        return None;
    }
    let secs = duration_str.parse::<f32>().ok()?;
    Some(secs.round() as u32)
}

/// Returns `true` if the media file at `path` contains both a video and an audio stream.
///
/// Uses `tokio::process::Command` with a 30-second timeout per probe.
pub async fn has_both_video_and_audio(path: &str) -> Result<bool, AppError> {
    use crate::core::process::{FFPROBE_TIMEOUT, run_with_timeout};

    let mut video_cmd = TokioCommand::new("ffprobe");
    video_cmd.args([
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "stream=codec_type",
        "-of",
        "default=noprint_wrappers=1:nokey=1",
        path,
    ]);
    let video_output = run_with_timeout(&mut video_cmd, FFPROBE_TIMEOUT)
        .await
        .map_err(|e| AppError::Download(DownloadError::Other(format!("Failed to check video stream: {}", e))))?;

    let has_video = !String::from_utf8_lossy(&video_output.stdout).trim().is_empty();

    let mut audio_cmd = TokioCommand::new("ffprobe");
    audio_cmd.args([
        "-v",
        "error",
        "-select_streams",
        "a:0",
        "-show_entries",
        "stream=codec_type",
        "-of",
        "default=noprint_wrappers=1:nokey=1",
        path,
    ]);
    let audio_output = run_with_timeout(&mut audio_cmd, FFPROBE_TIMEOUT)
        .await
        .map_err(|e| AppError::Download(DownloadError::Other(format!("Failed to check audio stream: {}", e))))?;

    let has_audio = !String::from_utf8_lossy(&audio_output.stdout).trim().is_empty();

    Ok(has_video && has_audio)
}

/// Probes video metadata: `(duration_seconds, width, height)`.
///
/// Used to supply the parameters Telegram requires when sending videos.
/// Uses `tokio::process::Command` with a 30-second timeout per probe.
///
/// **Rotation handling:** ffprobe's raw `stream=width,height` returns the
/// *coded* dimensions, ignoring display rotation. Portrait videos from
/// phones (iPhone, modern Android) are typically stored as `1920x1080 +
/// rotate=90`, and sending those raw dimensions to Telegram makes the client
/// render the video as landscape and stretch it to fit. We fix this by
/// asking ffprobe for rotation metadata in the same JSON call (both legacy
/// `tags.rotate` and modern `side_data_list[].rotation`) and swapping
/// width/height for 90°/270° rotations.
pub async fn probe_video_metadata(path: &str) -> Option<(u32, Option<u32>, Option<u32>)> {
    use crate::core::process::{FFPROBE_TIMEOUT, run_with_timeout};

    let duration = probe_duration_seconds(path).await?;

    // Single ffprobe call: width + height + rotation in one JSON roundtrip.
    let mut cmd = TokioCommand::new("ffprobe");
    cmd.args([
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "stream=width,height:stream_tags=rotate:stream_side_data=rotation",
        "-of",
        "json",
        path,
    ]);
    let output = run_with_timeout(&mut cmd, FFPROBE_TIMEOUT).await.ok()?;
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let (width, height) = dimensions_from_ffprobe_json(&json);

    Some((duration, width, height))
}

/// Extract display-oriented `(width, height)` from an ffprobe JSON payload,
/// swapping dimensions for 90°/270° rotated streams.
///
/// Pure function — factored out of `probe_video_metadata` so the rotation
/// logic is unit-testable without needing a real ffprobe subprocess.
///
/// Rotation sources checked (two conventions, both still seen in the wild):
///   1. `stream.tags.rotate` — legacy ffmpeg, string like `"90"` / `"-90"` / `"180"`
///   2. `stream.side_data_list[].rotation` — modern ffmpeg (≥4.3), signed int
pub(crate) fn dimensions_from_ffprobe_json(json: &serde_json::Value) -> (Option<u32>, Option<u32>) {
    let Some(stream) = json.get("streams").and_then(|s| s.as_array()).and_then(|a| a.first()) else {
        return (None, None);
    };

    let raw_w = stream
        .get("width")
        .and_then(serde_json::Value::as_u64)
        .map(|v| v as u32);
    let raw_h = stream
        .get("height")
        .and_then(serde_json::Value::as_u64)
        .map(|v| v as u32);

    let rotation_deg: i32 = stream
        .get("tags")
        .and_then(|t| t.get("rotate"))
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
        .or_else(|| {
            stream
                .get("side_data_list")?
                .as_array()?
                .iter()
                .find_map(|sd| sd.get("rotation").and_then(|r| r.as_i64()).map(|r| r as i32))
        })
        .unwrap_or(0);

    // Normalize to 0/90/180/270. Negative rotations (e.g. -90 for iPhone) are
    // equivalent to +270 for swap purposes.
    let normalized = rotation_deg.rem_euclid(360);

    if matches!(normalized, 90 | 270) {
        (raw_h, raw_w) // Portrait content stored with rotation applied — swap.
    } else {
        (raw_w, raw_h)
    }
}

/// Builds a yt-dlp format string optimised for Telegram compatibility.
///
/// Prioritises H.264/AAC (`avc1`/`mp4a`) codecs to ensure Telegram can play videos
/// correctly. Falls back through lower resolutions if the requested quality is
/// unavailable with compatible codecs.
///
/// Audio language selection is handled in post-processing via `replace_audio_track`
/// in `video.rs`, not via format string filters.
///
/// # Arguments
///
/// * `requested_height` — Optional preferred video height (e.g., `720`, `1080`)
///
/// # Examples
///
/// ```
/// use doracore::download::metadata::build_telegram_safe_format;
///
/// let fmt = build_telegram_safe_format(Some(1080));
/// assert!(fmt.contains("[height<=1080]"));
/// assert!(fmt.contains("avc1"));
///
/// let fmt = build_telegram_safe_format(None);
/// assert!(fmt.contains("avc1"));
/// assert!(fmt.ends_with("/best"));
/// ```
pub fn build_telegram_safe_format(requested_height: Option<u32>) -> String {
    let mut heights = vec![1080u32, 720, 480, 360, 240];
    if let Some(h) = requested_height {
        if !heights.contains(&h) {
            heights.insert(0, h);
        } else {
            heights.retain(|&v| v != h);
            heights.insert(0, h);
        }
    }

    let mut parts: Vec<String> = Vec::new();
    for h in &heights {
        let filt = format!("[height<={h}]");
        parts.push(format!("bv*{filt}[vcodec^=avc1]+ba[acodec^=mp4a]"));
        parts.push(format!("bv*{filt}[vcodec^=avc1][ext=mp4]+ba[ext=m4a]"));
    }

    parts.push("bestvideo[ext=mp4]+bestaudio[ext=m4a]".to_string());
    parts.push("best[ext=mp4]".to_string());
    parts.push("best".to_string());

    parts.join("/")
}

/// Builds a yt-dlp format string for high-resolution video (1440p/2160p/4320p).
///
/// YouTube serves H.264 (avc1) only up to 1080p, so for 2K/4K/8K we must accept
/// AV1 or VP9. Falls back to 1080p H.264 (via [`build_telegram_safe_format`]) if
/// the requested resolution is unavailable, ensuring the user still receives a
/// video rather than an error.
///
/// The output container should be `mkv` (pass `--merge-output-format mkv` to
/// yt-dlp) so AV1/VP9 video + AAC audio can be muxed without re-encoding.
pub fn build_highres_format(requested_height: u32) -> String {
    let filt = format!("[height<={}]", requested_height);
    let mut parts: Vec<String> = Vec::new();

    parts.push(format!("bv*{filt}[vcodec^=av01]+ba[acodec^=mp4a]"));
    parts.push(format!("bv*{filt}[vcodec^=av01]+ba"));
    parts.push(format!("bv*{filt}[vcodec^=vp9]+ba[acodec^=mp4a]"));
    parts.push(format!("bv*{filt}[vcodec^=vp9]+ba"));
    parts.push(format!("bv*{filt}+ba"));

    parts.push(build_telegram_safe_format(Some(1080)));

    parts.join("/")
}

/// Finds the actual downloaded file path after a yt-dlp download.
///
/// yt-dlp may append suffixes like `(1).mp4`, `(2).mp4` when a file already exists,
/// so this function scans the directory for the actual output file.
pub fn find_actual_downloaded_file(expected_path: &str) -> Result<String, AppError> {
    let path = Path::new(expected_path);

    if path.exists() {
        log::debug!("File found at expected path: {}", expected_path);
        return Ok(expected_path.to_string());
    }

    log::warn!("File not found at expected path: {}", expected_path);

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

            let matches_pattern =
                file_name_str.starts_with(file_stem) && file_name_str.ends_with(&format!(".{}", file_extension));

            if matches_pattern {
                found_files.push(entry.path().to_string_lossy().to_string());
            }
        }
    }

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

/// Callback for notifying administrators about yt-dlp errors.
///
/// Used by the bot to send error details via Telegram. The core library
/// logs errors at `error` level; callers that want richer notification
/// (e.g., sending a Telegram message) can supply this callback.
pub type ErrorNotifyFn =
    Box<dyn Fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync>;

/// Fetches metadata (title, artist) from yt-dlp for the given URL.
///
/// Checks the in-memory cache before invoking yt-dlp. When a yt-dlp error
/// requires administrator attention, the optional `error_notifier` callback
/// is invoked (in addition to logging at `error` level).
///
/// # Returns
///
/// `Ok((title, artist))` on success.
pub async fn get_metadata_from_ytdlp(
    url: &Url,
    error_notifier: Option<&ErrorNotifyFn>,
) -> Result<(String, String), AppError> {
    // Check cache; ignore "Unknown Track" or empty/NA artist entries
    if let Some((title, artist)) = cache::get_cached_metadata(url).await {
        if title.trim() != "Unknown Track" && !title.trim().is_empty() {
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
    let metadata_start = std::time::Instant::now();
    let ytdl_bin = &*config::YTDL_BIN;
    log::debug!("Using downloader binary: {}", ytdl_bin);

    let mut args_vec: Vec<String> = vec![
        "--print".to_string(),
        "%(title)s".to_string(),
        "--no-playlist".to_string(),
        "--playlist-items".to_string(),
        "1".to_string(),
        "--skip-download".to_string(),
    ];

    let mut temp_args: Vec<&str> = vec![];
    add_cookies_args(&mut temp_args);
    for arg in temp_args {
        args_vec.push(arg.to_string());
    }

    args_vec.push("--extractor-args".to_string());
    args_vec.push("youtube:player_client=android_vr,web_safari;formats=missing_pot".to_string());
    args_vec.push("--js-runtimes".to_string());
    args_vec.push("deno".to_string());
    args_vec.push("--no-check-certificate".to_string());
    args_vec.push(url.as_str().to_string());

    let args: Vec<&str> = args_vec.iter().map(|s| s.as_str()).collect();

    let command_str = format!("{} {}", ytdl_bin, args.join(" "));
    log::debug!("yt-dlp command for metadata: {}", command_str);

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
        metrics::METADATA_FETCH_DURATION_SECONDS.observe(metadata_start.elapsed().as_secs_f64());
        AppError::Download(DownloadError::YtDlp("yt-dlp command timed out".to_string()))
    })?
    .map_err(|e| {
        log::error!("Failed to execute {}: {}", ytdl_bin, e);
        metrics::record_error("download", "metadata_spawn");
        metrics::METADATA_FETCH_DURATION_SECONDS.observe(metadata_start.elapsed().as_secs_f64());
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

        log::error!("yt-dlp failed to get metadata, error type: {:?}", error_type);
        log::error!("yt-dlp stderr: {}", stderr);

        if should_notify_admin(&error_type) {
            log::error!(
                "Admin attention required — YTDLP ERROR (metadata): url={}, error_type={:?}, command={}, stderr={}",
                url,
                error_type,
                command_str,
                stderr,
            );

            if let Some(notifier) = error_notifier {
                let mut text = String::new();
                text.push_str("YTDLP ERROR (metadata)\n");
                text.push_str(&format!("url: {}\n", url));
                text.push_str(&format!("error_type: {:?}\n\n", error_type));
                text.push_str("command:\n");
                text.push_str(&command_str);
                text.push_str("\n\nstderr:\n");
                text.push_str(&stderr);
                notifier(text).await;
            }
        }

        metrics::METADATA_FETCH_DURATION_SECONDS.observe(metadata_start.elapsed().as_secs_f64());
        return Err(AppError::Download(DownloadError::YtDlp(get_error_message(&error_type))));
    }

    let title = first_line_of_stdout(&title_output.stdout);

    if title.is_empty() {
        log::error!("yt-dlp returned empty title for URL: {}", url);
        metrics::record_error("download", "metadata_empty_title");
        metrics::METADATA_FETCH_DURATION_SECONDS.observe(metadata_start.elapsed().as_secs_f64());
        return Err(AppError::Download(DownloadError::YtDlp(
            "Failed to get video title. Video might be unavailable or private.".to_string(),
        )));
    }

    log::info!("Successfully got metadata from yt-dlp: title='{}'", title);

    // Fetch artist via --print "%(artist)s"
    let mut artist_args_vec: Vec<String> = vec![
        "--print".to_string(),
        "%(artist)s".to_string(),
        "--no-playlist".to_string(),
        "--playlist-items".to_string(),
        "1".to_string(),
        "--skip-download".to_string(),
    ];

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
    .ok();

    let mut artist = artist_output
        .and_then(|result| result.ok())
        .map(|out| first_line_of_stdout(&out.stdout))
        .unwrap_or_default();

    // If artist is empty or "NA", fall back to uploader/channel name
    if artist.trim().is_empty() || artist.trim() == "NA" {
        log::debug!("Artist is empty or 'NA', trying to get channel/uploader");

        let mut uploader_args_vec: Vec<String> = vec![
            "--print".to_string(),
            "%(uploader)s".to_string(),
            "--no-playlist".to_string(),
            "--playlist-items".to_string(),
            "1".to_string(),
            "--skip-download".to_string(),
        ];

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
            .map(|out| first_line_of_stdout(&out.stdout))
            .unwrap_or_default();

        if !uploader.trim().is_empty() && uploader.trim() != "NA" {
            artist = uploader;
            log::info!("Using uploader/channel as artist: '{}'", artist);
        } else {
            log::warn!("Could not get artist or uploader, leaving empty");
        }
    }

    if !title.trim().is_empty() && title.trim() != "Unknown Track" && !title.contains('\n') && title.len() <= 300 {
        cache::cache_metadata(url, title.clone(), artist.clone()).await;
    } else {
        log::warn!("Not caching metadata with invalid title: '{}'", title);
    }

    log::info!("Got metadata from yt-dlp: title='{}', artist='{}'", title, artist);
    metrics::METADATA_FETCH_DURATION_SECONDS.observe(metadata_start.elapsed().as_secs_f64());
    Ok((title, artist))
}

/// Returns the estimated file size in bytes for the given URL, or `None` if unavailable.
///
/// Adds 15% overhead to account for conversion/encoding during post-processing.
/// Used to reject downloads that would exceed size limits before starting the download.
pub async fn get_estimated_filesize(url: &Url) -> Option<u64> {
    let ytdl_bin = &*config::YTDL_BIN;

    let mut args_vec: Vec<String> = vec![
        "--print".to_string(),
        "%(filesize_approx)s".to_string(),
        "--no-playlist".to_string(),
        "--playlist-items".to_string(),
        "1".to_string(),
        "--skip-download".to_string(),
    ];

    let mut temp_args: Vec<&str> = vec![];
    add_cookies_args(&mut temp_args);
    for arg in temp_args {
        args_vec.push(arg.to_string());
    }

    args_vec.push("--no-check-certificate".to_string());
    args_vec.push(url.as_str().to_string());

    let args: Vec<&str> = args_vec.iter().map(|s| s.as_str()).collect();

    log::debug!("Getting estimated filesize for URL: {}", url);

    let output = timeout(
        std::time::Duration::from_secs(30),
        TokioCommand::new(ytdl_bin).args(&args).output(),
    )
    .await;

    match output {
        Ok(Ok(result)) if result.status.success() => {
            let size_str = first_line_of_stdout(&result.stdout);
            if size_str == "NA" || size_str.is_empty() {
                log::debug!("File size not available for URL: {}", url);
                return None;
            }
            match size_str.parse::<u64>() {
                Ok(size) => {
                    let size_with_overhead = (size as f64 * 1.15) as u64;
                    log::info!(
                        "Estimated file size for {}: {:.2} MB (with 15% overhead: {:.2} MB)",
                        url,
                        size as f64 / (1024.0 * 1024.0),
                        size_with_overhead as f64 / (1024.0 * 1024.0),
                    );
                    Some(size_with_overhead)
                }
                Err(_) => {
                    log::debug!("Failed to parse file size '{}' for URL: {}", size_str, url);
                    None
                }
            }
        }
        _ => {
            log::debug!("Could not get estimated filesize for: {}", url);
            None
        }
    }
}

/// Fast livestream check using cached info JSON (experimental mode).
///
/// Returns `Some(true)` if live, `Some(false)` if not live, `None` on cache miss.
/// The cache file is written during the preview phase at `/tmp/ytdlp-info-{id}.json`.
/// Avoids a ~6.5s yt-dlp network call when the info is already on disk.
pub fn check_is_live_from_cache(url: &Url) -> Option<bool> {
    let cache_path = crate::core::share::youtube_info_cache_path(url.as_str())?;
    let content = fs::read_to_string(&cache_path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    // yt-dlp sets is_live to null for non-live videos, false when explicitly not live,
    // and true when live. Treat null/missing as false.
    let is_live = json.get("is_live").and_then(|v| v.as_bool()).unwrap_or(false);
    Some(is_live)
}

/// Returns `true` if the URL points to a live stream.
///
/// Used to reject live stream URLs before starting a download.
pub async fn is_livestream(url: &Url) -> bool {
    let ytdl_bin = &*config::YTDL_BIN;

    let mut args_vec: Vec<String> = vec![
        "--print".to_string(),
        "%(is_live)s".to_string(),
        "--no-playlist".to_string(),
        "--playlist-items".to_string(),
        "1".to_string(),
        "--skip-download".to_string(),
    ];

    let mut temp_args: Vec<&str> = vec![];
    add_cookies_args(&mut temp_args);
    for arg in temp_args {
        args_vec.push(arg.to_string());
    }

    args_vec.push("--no-check-certificate".to_string());
    args_vec.push(url.as_str().to_string());

    let args: Vec<&str> = args_vec.iter().map(|s| s.as_str()).collect();

    log::debug!("Checking if URL is livestream: {}", url);

    let output = timeout(
        std::time::Duration::from_secs(30),
        TokioCommand::new(ytdl_bin).args(&args).output(),
    )
    .await;

    match output {
        Ok(Ok(result)) if result.status.success() => {
            let is_live_str = first_line_of_stdout(&result.stdout).to_lowercase();
            let is_live = is_live_str == "true" || is_live_str == "1";
            if is_live {
                log::warn!("URL is a LIVE STREAM, will be rejected: {}", url);
            } else {
                log::debug!("URL is not a livestream (is_live={})", is_live_str);
            }
            is_live
        }
        Ok(Ok(result)) => {
            let stderr = String::from_utf8_lossy(&result.stderr);
            if stderr.contains("live") || stderr.contains("is live") {
                log::warn!("URL appears to be a livestream based on error: {}", url);
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

#[cfg(test)]
mod tests {
    use super::{dimensions_from_ffprobe_json, first_line_of_stdout};
    use serde_json::json;

    // ── first_line_of_stdout ──────────────────────────────────────────────

    #[test]
    fn single_line() {
        assert_eq!(first_line_of_stdout(b"hello world"), "hello world");
    }

    #[test]
    fn multi_line_takes_first() {
        assert_eq!(first_line_of_stdout(b"Track1\nTrack2\nTrack3"), "Track1");
    }

    #[test]
    fn empty_input() {
        assert_eq!(first_line_of_stdout(b""), "");
    }

    #[test]
    fn crlf_takes_first() {
        assert_eq!(first_line_of_stdout(b"Song\r\nTitle"), "Song");
    }

    #[test]
    fn whitespace_trimmed() {
        assert_eq!(first_line_of_stdout(b"  padded  \nmore"), "padded");
    }

    // ── dimensions_from_ffprobe_json ──────────────────────────────────────

    #[test]
    fn landscape_no_rotation_not_swapped() {
        // Standard 1920x1080 landscape video, no rotation metadata at all.
        let j = json!({
            "streams": [{ "width": 1920, "height": 1080 }]
        });
        assert_eq!(dimensions_from_ffprobe_json(&j), (Some(1920), Some(1080)));
    }

    #[test]
    fn native_portrait_no_rotation_not_swapped() {
        // Video authored in portrait (e.g. TikTok re-encoded) — raw dimensions
        // are already portrait, no rotation metadata. Should stay as-is.
        let j = json!({
            "streams": [{ "width": 1080, "height": 1920, "tags": {} }]
        });
        assert_eq!(dimensions_from_ffprobe_json(&j), (Some(1080), Some(1920)));
    }

    #[test]
    fn legacy_rotate_tag_90_swaps() {
        // Old ffmpeg convention: raw 1920x1080 + `tags.rotate = "90"` → display
        // as 1080x1920 portrait. This is the bug that made iPhone videos appear
        // stretched to landscape in Telegram.
        let j = json!({
            "streams": [{
                "width": 1920,
                "height": 1080,
                "tags": { "rotate": "90" }
            }]
        });
        assert_eq!(dimensions_from_ffprobe_json(&j), (Some(1080), Some(1920)));
    }

    #[test]
    fn legacy_rotate_tag_minus_90_swaps() {
        // iPhones write `rotate=-90` instead of `270`; normalization must swap.
        let j = json!({
            "streams": [{
                "width": 1920,
                "height": 1080,
                "tags": { "rotate": "-90" }
            }]
        });
        assert_eq!(dimensions_from_ffprobe_json(&j), (Some(1080), Some(1920)));
    }

    #[test]
    fn legacy_rotate_tag_180_does_not_swap() {
        // Upside-down landscape — still landscape dimensions after rotation.
        let j = json!({
            "streams": [{
                "width": 1920,
                "height": 1080,
                "tags": { "rotate": "180" }
            }]
        });
        assert_eq!(dimensions_from_ffprobe_json(&j), (Some(1920), Some(1080)));
    }

    #[test]
    fn modern_display_matrix_rotation_swaps() {
        // Modern ffmpeg (≥4.3) records rotation in `side_data_list` under a
        // "Display Matrix" side-data entry, not in `tags.rotate`. iPhone 14+
        // and Android 12+ both use this convention.
        let j = json!({
            "streams": [{
                "width": 1920,
                "height": 1080,
                "side_data_list": [
                    {
                        "side_data_type": "Display Matrix",
                        "displaymatrix": "...",
                        "rotation": -90
                    }
                ]
            }]
        });
        assert_eq!(dimensions_from_ffprobe_json(&j), (Some(1080), Some(1920)));
    }

    #[test]
    fn legacy_tag_takes_precedence_over_side_data() {
        // If both are present (transcoded files sometimes carry both), the
        // legacy tag is checked first — either would yield the correct swap.
        let j = json!({
            "streams": [{
                "width": 1920,
                "height": 1080,
                "tags": { "rotate": "90" },
                "side_data_list": [{ "rotation": -90 }]
            }]
        });
        assert_eq!(dimensions_from_ffprobe_json(&j), (Some(1080), Some(1920)));
    }

    #[test]
    fn missing_streams_returns_none() {
        let j = json!({ "streams": [] });
        assert_eq!(dimensions_from_ffprobe_json(&j), (None, None));
    }

    #[test]
    fn garbage_rotation_string_falls_back_to_zero() {
        // Unparseable rotate tag → treated as no rotation → no swap.
        let j = json!({
            "streams": [{
                "width": 1920,
                "height": 1080,
                "tags": { "rotate": "garbage" }
            }]
        });
        assert_eq!(dimensions_from_ffprobe_json(&j), (Some(1920), Some(1080)));
    }
}
