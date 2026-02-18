/// Module for analyzing yt-dlp errors
///
/// Provides functions for determining the yt-dlp error type
/// and generating informative messages for the user and administrator.
/// yt-dlp error types
#[derive(Debug, Clone, PartialEq)]
pub enum YtDlpErrorType {
    /// Cookies are invalid or outdated
    InvalidCookies,
    /// YouTube detected a bot
    BotDetection,
    /// Video is unavailable (private, deleted, regional restrictions)
    VideoUnavailable,
    /// Network issues (timeouts, connection)
    NetworkError,
    /// Errors while downloading video fragments (usually temporary)
    FragmentError,
    /// Post-processing error (ffmpeg FixupM3u8, conversion, etc.)
    PostprocessingError,
    /// Insufficient disk space
    DiskSpaceError,
    /// Unknown error
    Unknown,
}

/// Analyzes yt-dlp stderr and determines the error type
///
/// # Parameters
/// - `stderr`: stderr output from yt-dlp
///
/// # Returns
/// - `YtDlpErrorType`: the determined error type
pub fn analyze_ytdlp_error(stderr: &str) -> YtDlpErrorType {
    let stderr_lower = stderr.to_lowercase();

    // Bot detection: YouTube requires confirmation that you are not a bot
    // This is NOT a cookies problem — it is an IP/fingerprint block
    if stderr_lower.contains("sign in to confirm you're not a bot")
        || stderr_lower.contains("sign in to confirm you\u{2019}re not a bot")
        || stderr_lower.contains("confirm you're not a bot")
        || stderr_lower.contains("confirm you\u{2019}re not a bot")
    {
        return YtDlpErrorType::BotDetection;
    }

    // Check for cookie-related errors (genuinely invalid cookies)
    if stderr_lower.contains("cookies are no longer valid")
        || stderr_lower.contains("cookies have likely been rotated")
        || stderr_lower.contains("please sign in")
        || stderr_lower.contains("use --cookies-from-browser")
        || stderr_lower.contains("use --cookies for the authentication")
        || stderr_lower.contains("the provided youtube account cookies are no longer valid")
    {
        return YtDlpErrorType::InvalidCookies;
    }

    // Check for fragment download errors (usually temporary blocks)
    if stderr_lower.contains("fragment")
        && (stderr_lower.contains("http error 403")
            || stderr_lower.contains("retrying fragment")
            || stderr_lower.contains("fragment not found")
            || stderr_lower.contains("skipping fragment"))
    {
        return YtDlpErrorType::FragmentError;
    }

    // Check for bot detection (if not fragment-related)
    if stderr_lower.contains("bot detection")
        || stderr_lower.contains("http error 403")
        || stderr_lower.contains("unable to extract")
        || stderr_lower.contains("signature extraction failed")
    {
        return YtDlpErrorType::BotDetection;
    }

    // Check for unavailable video
    if stderr_lower.contains("private video")
        || stderr_lower.contains("video unavailable")
        || stderr_lower.contains("this video is not available")
        || stderr_lower.contains("video is private")
        || stderr_lower.contains("video has been removed")
        || stderr_lower.contains("this video does not exist")
        || stderr_lower.contains("video is not available")
    {
        return YtDlpErrorType::VideoUnavailable;
    }

    // Check for network errors
    if stderr_lower.contains("timeout")
        || stderr_lower.contains("connection")
        || stderr_lower.contains("network")
        || stderr_lower.contains("socket")
        || stderr_lower.contains("dns")
        || stderr_lower.contains("failed to connect")
    {
        return YtDlpErrorType::NetworkError;
    }

    // Check for post-processing errors (ffmpeg, FixupM3u8, etc.)
    if stderr_lower.contains("postprocessing")
        || stderr_lower.contains("conversion failed")
        || stderr_lower.contains("fixupm3u8")
        || stderr_lower.contains("ffmpeg")
        || stderr_lower.contains("merger")
        || stderr_lower.contains("error fixing")
    {
        return YtDlpErrorType::PostprocessingError;
    }

    // Check for disk space errors
    if stderr_lower.contains("no space left")
        || stderr_lower.contains("disk quota")
        || stderr_lower.contains("not enough space")
        || stderr_lower.contains("insufficient disk space")
        || stderr_lower.contains("enospc")
        || stderr_lower.contains("no free space")
        || stderr_lower.contains("disk full")
    {
        return YtDlpErrorType::DiskSpaceError;
    }

    // Unknown error
    YtDlpErrorType::Unknown
}

/// Returns the user-facing error message
///
/// # Parameters
/// - `error_type`: the error type
///
/// # Returns
/// - `String`: message for the user
pub fn get_error_message(error_type: &YtDlpErrorType) -> String {
    match error_type {
        YtDlpErrorType::InvalidCookies => {
            "❌ Temporary issue with YouTube.\n\nTry a different video or retry later.".to_string()
        }
        YtDlpErrorType::BotDetection => {
            "❌ YouTube blocked the request.\n\nTry a different video or retry later.".to_string()
        }
        YtDlpErrorType::VideoUnavailable => {
            "❌ Video unavailable.\n\nIt may be private, deleted, or blocked in your region.".to_string()
        }
        YtDlpErrorType::NetworkError => "❌ Network problem.\n\nTry again in a minute.".to_string(),
        YtDlpErrorType::FragmentError => "❌ Temporary issue while downloading video.\n\nPlease retry.".to_string(),
        YtDlpErrorType::PostprocessingError => "❌ Video processing error.\n\nPlease retry.".to_string(),
        YtDlpErrorType::DiskSpaceError => {
            "❌ Server is overloaded.\n\nTry again later — we are already working on it.".to_string()
        }
        YtDlpErrorType::Unknown => "❌ Failed to download video.\n\nCheck that the link is correct.".to_string(),
    }
}

/// Determines whether the administrator should be notified about the error
///
/// # Parameters
/// - `error_type`: the error type
///
/// # Returns
/// - `true` if the administrator should be notified
pub fn should_notify_admin(error_type: &YtDlpErrorType) -> bool {
    match error_type {
        YtDlpErrorType::InvalidCookies => true,
        YtDlpErrorType::BotDetection => true,
        YtDlpErrorType::VideoUnavailable => false,
        YtDlpErrorType::NetworkError => false,
        YtDlpErrorType::FragmentError => false, // Temporary fragment errors - no action needed
        YtDlpErrorType::PostprocessingError => false, // Retried with --fixup never
        YtDlpErrorType::DiskSpaceError => true, // CRITICAL: disk space must be freed immediately!
        YtDlpErrorType::Unknown => true,
    }
}

/// Sanitizes a raw error string for user-facing output.
///
/// If the message looks like a yt-dlp stderr dump, return a friendly
/// user message instead of the raw error text.
pub fn sanitize_user_error_message(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "❌ Failed to download video.\n\nPlease try again later.".to_string();
    }

    let lower = trimmed.to_lowercase();
    let looks_like_ytdlp = lower.contains("yt-dlp")
        || lower.contains("youtube-dl")
        || lower.contains("http error 403")
        || lower.contains("fragment")
        || lower.contains("signature extraction")
        || lower.contains("bot detection")
        || lower.contains("stderr")
        || lower.contains("stdout")
        || lower.contains("recommendations")
        || lower.contains("[download]")
        || lower.contains("warning: [youtube]")
        || lower.contains("error: [youtube]")
        || lower.contains("downloaded file is empty")
        || lower.contains("unable to download")
        || lower.contains("sign in to confirm you're not a bot")
        || lower.contains("sign in to confirm you’re not a bot")
        || lower.contains("confirm you're not a bot")
        || lower.contains("confirm you’re not a bot");

    if looks_like_ytdlp {
        let error_type = analyze_ytdlp_error(trimmed);
        return get_error_message(&error_type);
    }

    trimmed.to_string()
}

/// Returns fix recommendations for the logs
///
/// # Parameters
/// - `error_type`: the error type
///
/// # Returns
/// - `String`: recommendations for the administrator
pub fn get_fix_recommendations(error_type: &YtDlpErrorType) -> String {
    match error_type {
        YtDlpErrorType::InvalidCookies => "🔧 FIX RECOMMENDATIONS:\n\
            • Cookies are outdated or were refreshed in the browser\n\
            \n\
            📋 Option 1: Automatic extraction from browser (recommended for Linux/Windows):\n\
              1. Make sure you are logged in to youtube.com in the browser\n\
              2. Install dependencies: pip3 install keyring pycryptodomex\n\
              3. Set the variable: export YTDL_COOKIES_BROWSER=chrome\n\
                 (supported: chrome, firefox, safari, brave, chromium, edge, opera, vivaldi)\n\
              4. Restart the bot\n\
            \n\
            📋 Option 2: Export cookies to a file (recommended for macOS):\n\
              1. Open the browser and log in to youtube.com\n\
              2. Export cookies to a file youtube_cookies.txt\n\
              3. Ensure the file is in Netscape HTTP Cookie File format\n\
              4. Set the variable: export YTDL_COOKIES_FILE=youtube_cookies.txt\n\
              5. Restart the bot"
            .to_string(),
        YtDlpErrorType::BotDetection => "🔧 FIX RECOMMENDATIONS:\n\
            • YouTube detected automated requests\n\
            • Update cookies from the browser\n\
            • Ensure you are using an up-to-date version of yt-dlp\n\
            • Try using a different player_client (android, web)"
            .to_string(),
        YtDlpErrorType::VideoUnavailable => {
            "ℹ️  Video unavailable - this is a normal situation, no action required".to_string()
        }
        YtDlpErrorType::NetworkError => "🔧 FIX RECOMMENDATIONS:\n\
            • Check your internet connection\n\
            • Check accessibility of youtube.com\n\
            • Increase timeouts if the problem persists"
            .to_string(),
        YtDlpErrorType::FragmentError => "🔧 FIX RECOMMENDATIONS:\n\
            • This is a temporary error while downloading video - yt-dlp retries fragments automatically\n\
            • If the problem occurs frequently:\n\
              1. Check internet connection\n\
              2. Try downloading later (YouTube may be rate-limiting frequent requests)\n\
              3. Ensure you are using an up-to-date version of yt-dlp"
            .to_string(),
        YtDlpErrorType::PostprocessingError => "🔧 FIX RECOMMENDATIONS:\n\
            • Video post-processing error (ffmpeg/FixupM3u8)\n\
            • The bot will automatically retry without post-processing\n\
            • If the problem persists:\n\
              1. Check the ffmpeg version\n\
              2. Check available disk space\n\
              3. Check write permissions for /tmp"
            .to_string(),
        YtDlpErrorType::DiskSpaceError => "🚨 CRITICAL - DISK SPACE SHORTAGE:\n\
            • Downloads will fail until space is freed!\n\
            \n\
            📋 URGENT ACTIONS:\n\
              1. Check disk: df -h\n\
              2. Clear downloads/: rm -rf /app/downloads/*\n\
              3. Clear /tmp: rm -rf /tmp/*\n\
              4. Check logs: du -sh /app/logs/*\n\
              5. If Railway — increase disk size in the settings"
            .to_string(),
        YtDlpErrorType::Unknown => "🔧 FIX RECOMMENDATIONS:\n\
            • Check yt-dlp logs for details\n\
            • Ensure the video is accessible\n\
            • Check that yt-dlp is updated to the latest version"
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== analyze_ytdlp_error Tests ====================

    #[test]
    fn test_analyze_invalid_cookies_error() {
        let cases = vec![
            "cookies are no longer valid",
            "Cookies have likely been rotated",
            "Please sign in",
            "Use --cookies-from-browser",
            "Use --cookies for the authentication",
            "The provided YouTube account cookies are no longer valid",
        ];

        for case in cases {
            assert_eq!(
                analyze_ytdlp_error(case),
                YtDlpErrorType::InvalidCookies,
                "Failed for: {}",
                case
            );
        }
    }

    #[test]
    fn test_analyze_bot_detection_error() {
        let cases = vec![
            "bot detection triggered",
            "HTTP Error 403: Forbidden",
            "Unable to extract video data",
            "Signature extraction failed",
            "Sign in to confirm you're not a bot",
            "confirm you're not a bot",
        ];

        for case in cases {
            assert_eq!(
                analyze_ytdlp_error(case),
                YtDlpErrorType::BotDetection,
                "Failed for: {}",
                case
            );
        }
    }

    #[test]
    fn test_analyze_video_unavailable_error() {
        let cases = vec![
            "Private video",
            "Video unavailable",
            "This video is not available in your country",
            "Video is private",
            "Video has been removed",
            "This video does not exist",
            "Video is not available",
        ];

        for case in cases {
            assert_eq!(
                analyze_ytdlp_error(case),
                YtDlpErrorType::VideoUnavailable,
                "Failed for: {}",
                case
            );
        }
    }

    #[test]
    fn test_analyze_network_error() {
        let cases = vec![
            "Connection timeout",
            "Connection refused",
            "Network unreachable",
            "Socket error",
            "DNS resolution failed",
            "Failed to connect to server",
        ];

        for case in cases {
            assert_eq!(
                analyze_ytdlp_error(case),
                YtDlpErrorType::NetworkError,
                "Failed for: {}",
                case
            );
        }
    }

    #[test]
    fn test_analyze_unknown_error() {
        let cases = vec!["Some random error", "Unknown error occurred", "Unexpected behavior", ""];

        for case in cases {
            assert_eq!(
                analyze_ytdlp_error(case),
                YtDlpErrorType::Unknown,
                "Failed for: '{}'",
                case
            );
        }
    }

    #[test]
    fn test_analyze_case_insensitive() {
        // Should work regardless of case
        assert_eq!(
            analyze_ytdlp_error("COOKIES ARE NO LONGER VALID"),
            YtDlpErrorType::InvalidCookies
        );
        assert_eq!(analyze_ytdlp_error("http error 403"), YtDlpErrorType::BotDetection);
        assert_eq!(analyze_ytdlp_error("PRIVATE VIDEO"), YtDlpErrorType::VideoUnavailable);
        assert_eq!(analyze_ytdlp_error("CONNECTION TIMEOUT"), YtDlpErrorType::NetworkError);
    }

    // ==================== get_error_message Tests ====================

    #[test]
    fn test_get_error_message_invalid_cookies() {
        let msg = get_error_message(&YtDlpErrorType::InvalidCookies);
        assert!(msg.contains("❌"));
        assert!(msg.contains("YouTube"));
    }

    #[test]
    fn test_get_error_message_bot_detection() {
        let msg = get_error_message(&YtDlpErrorType::BotDetection);
        assert!(msg.contains("❌"));
        assert!(msg.contains("YouTube"));
        assert!(msg.contains("blocked"));
    }

    #[test]
    fn test_get_error_message_video_unavailable() {
        let msg = get_error_message(&YtDlpErrorType::VideoUnavailable);
        assert!(msg.contains("❌"));
        assert!(msg.contains("unavailable"));
    }

    #[test]
    fn test_get_error_message_network() {
        let msg = get_error_message(&YtDlpErrorType::NetworkError);
        assert!(msg.contains("❌"));
        assert!(msg.contains("Network"));
    }

    #[test]
    fn test_get_error_message_unknown() {
        let msg = get_error_message(&YtDlpErrorType::Unknown);
        assert!(msg.contains("❌"));
        assert!(msg.contains("download"));
    }

    // ==================== should_notify_admin Tests ====================

    #[test]
    fn test_should_notify_admin_critical_errors() {
        assert!(should_notify_admin(&YtDlpErrorType::InvalidCookies));
        assert!(should_notify_admin(&YtDlpErrorType::BotDetection));
        assert!(should_notify_admin(&YtDlpErrorType::Unknown));
    }

    #[test]
    fn test_should_not_notify_admin_normal_errors() {
        assert!(!should_notify_admin(&YtDlpErrorType::VideoUnavailable));
        assert!(!should_notify_admin(&YtDlpErrorType::NetworkError));
    }

    // ==================== get_fix_recommendations Tests ====================

    #[test]
    fn test_get_fix_recommendations_invalid_cookies() {
        let recs = get_fix_recommendations(&YtDlpErrorType::InvalidCookies);
        assert!(recs.contains("RECOMMENDATIONS"));
        assert!(recs.contains("cookies"));
        assert!(recs.contains("browser"));
    }

    #[test]
    fn test_get_fix_recommendations_bot_detection() {
        let recs = get_fix_recommendations(&YtDlpErrorType::BotDetection);
        assert!(recs.contains("RECOMMENDATIONS"));
        assert!(recs.contains("yt-dlp"));
    }

    #[test]
    fn test_get_fix_recommendations_video_unavailable() {
        let recs = get_fix_recommendations(&YtDlpErrorType::VideoUnavailable);
        assert!(recs.contains("unavailable"));
        assert!(recs.contains("no action"));
    }

    #[test]
    fn test_get_fix_recommendations_network() {
        let recs = get_fix_recommendations(&YtDlpErrorType::NetworkError);
        assert!(recs.contains("internet"));
        assert!(recs.contains("youtube.com"));
    }

    #[test]
    fn test_get_fix_recommendations_unknown() {
        let recs = get_fix_recommendations(&YtDlpErrorType::Unknown);
        assert!(recs.contains("logs"));
        assert!(recs.contains("yt-dlp"));
    }

    // ==================== sanitize_user_error_message Tests ====================

    #[test]
    fn test_sanitize_user_error_message_ytdlp() {
        let raw = "ERROR: [youtube] abc: HTTP Error 403: Forbidden";
        let sanitized = sanitize_user_error_message(raw);
        assert!(!sanitized.to_lowercase().contains("yt-dlp"));
        assert!(sanitized.contains("YouTube"));
    }

    #[test]
    fn test_sanitize_user_error_message_passthrough() {
        let raw = "❌ Video unavailable.\n\nTry a different video.";
        let sanitized = sanitize_user_error_message(raw);
        assert_eq!(sanitized, raw);
    }

    // ==================== YtDlpErrorType Trait Tests ====================

    #[test]
    fn test_error_type_debug() {
        assert_eq!(format!("{:?}", YtDlpErrorType::InvalidCookies), "InvalidCookies");
        assert_eq!(format!("{:?}", YtDlpErrorType::BotDetection), "BotDetection");
        assert_eq!(format!("{:?}", YtDlpErrorType::VideoUnavailable), "VideoUnavailable");
        assert_eq!(format!("{:?}", YtDlpErrorType::NetworkError), "NetworkError");
        assert_eq!(format!("{:?}", YtDlpErrorType::Unknown), "Unknown");
    }

    #[test]
    fn test_error_type_clone() {
        let original = YtDlpErrorType::InvalidCookies;
        let cloned = original.clone();
        assert_eq!(original, cloned);
    }

    #[test]
    fn test_error_type_equality() {
        assert_eq!(YtDlpErrorType::InvalidCookies, YtDlpErrorType::InvalidCookies);
        assert_ne!(YtDlpErrorType::InvalidCookies, YtDlpErrorType::BotDetection);
        assert_ne!(YtDlpErrorType::NetworkError, YtDlpErrorType::Unknown);
    }

    // ==================== Integration Tests ====================

    #[test]
    fn test_full_error_handling_flow() {
        let stderr = "ERROR: Cookies are no longer valid. Please use --cookies-from-browser";

        let error_type = analyze_ytdlp_error(stderr);
        assert_eq!(error_type, YtDlpErrorType::InvalidCookies);

        let user_msg = get_error_message(&error_type);
        assert!(!user_msg.is_empty());

        let notify = should_notify_admin(&error_type);
        assert!(notify);

        let recommendations = get_fix_recommendations(&error_type);
        assert!(recommendations.contains("cookies"));
    }

    #[test]
    fn test_real_world_error_messages() {
        // Real error messages from yt-dlp
        let cases = vec![
            (
                "ERROR: [youtube] dQw4w9WgXcQ: Sign in to confirm you're not a bot. Use --cookies-from-browser",
                YtDlpErrorType::BotDetection,
            ),
            (
                "ERROR: [youtube] abc123: Private video. Sign in if you've been granted access to this video",
                YtDlpErrorType::VideoUnavailable,
            ),
            (
                "ERROR: unable to download video data: HTTP Error 403: Forbidden",
                YtDlpErrorType::BotDetection,
            ),
            // Note: "timed out" matches "timeout" check since we use contains
            (
                "ERROR: Unable to download webpage: Connection timeout",
                YtDlpErrorType::NetworkError,
            ),
        ];

        for (stderr, expected) in cases {
            assert_eq!(analyze_ytdlp_error(stderr), expected, "Failed for stderr: {}", stderr);
        }
    }
}
