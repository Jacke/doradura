use std::fmt;

/// Structured error type for download operations.
///
/// Replaces the previous `AppError::Download(String)` with categorized variants
/// for better error handling, metrics, and debugging.
#[derive(Debug)]
pub enum DownloadError {
    /// yt-dlp specific failures (binary not found, bad exit code, etc.)
    YtDlp(String),
    /// FFmpeg processing failures (encoding, splitting, subtitle burn)
    Ffmpeg(String),
    /// Expected file not found after processing
    FileNotFound(String),
    /// Download or processing timed out
    Timeout(String),
    /// Proxy configuration or connection error
    Proxy(String),
    /// Insufficient disk space
    DiskSpace(String),
    /// Failed to send file via Telegram API
    SendFailed(String),
    /// Process execution failure (spawn, exit code)
    Process(String),
    /// Instagram API specific failures (GraphQL errors, doc_id expiry, private accounts)
    Instagram(String),
    /// Catch-all for uncategorized errors
    Other(String),
}

impl fmt::Display for DownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DownloadError::YtDlp(msg) => write!(f, "{}", msg),
            DownloadError::Ffmpeg(msg) => write!(f, "{}", msg),
            DownloadError::FileNotFound(msg) => write!(f, "{}", msg),
            DownloadError::Timeout(msg) => write!(f, "{}", msg),
            DownloadError::Proxy(msg) => write!(f, "{}", msg),
            DownloadError::DiskSpace(msg) => write!(f, "{}", msg),
            DownloadError::SendFailed(msg) => write!(f, "{}", msg),
            DownloadError::Process(msg) => write!(f, "{}", msg),
            DownloadError::Instagram(msg) => write!(f, "{}", msg),
            DownloadError::Other(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for DownloadError {}

impl DownloadError {
    /// Returns subcategory for metrics
    pub fn subcategory(&self) -> &'static str {
        match self {
            DownloadError::YtDlp(_) => "ytdlp",
            DownloadError::Ffmpeg(_) => "ffmpeg",
            DownloadError::FileNotFound(_) => "file_not_found",
            DownloadError::Timeout(_) => "timeout",
            DownloadError::Proxy(_) => "proxy",
            DownloadError::DiskSpace(_) => "disk_space",
            DownloadError::SendFailed(_) => "send_failed",
            DownloadError::Process(_) => "process",
            DownloadError::Instagram(_) => "instagram",
            DownloadError::Other(_) => "other",
        }
    }

    /// Returns the inner message
    pub fn message(&self) -> &str {
        match self {
            DownloadError::YtDlp(msg)
            | DownloadError::Ffmpeg(msg)
            | DownloadError::FileNotFound(msg)
            | DownloadError::Timeout(msg)
            | DownloadError::Proxy(msg)
            | DownloadError::DiskSpace(msg)
            | DownloadError::SendFailed(msg)
            | DownloadError::Process(msg)
            | DownloadError::Instagram(msg)
            | DownloadError::Other(msg) => msg,
        }
    }
}

/// Backwards compatibility: plain strings become `DownloadError::Other`
impl From<String> for DownloadError {
    fn from(s: String) -> Self {
        DownloadError::Other(s)
    }
}

impl From<&str> for DownloadError {
    fn from(s: &str) -> Self {
        DownloadError::Other(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_download_error_display() {
        let err = DownloadError::YtDlp("yt-dlp failed".into());
        assert_eq!(err.to_string(), "yt-dlp failed");
    }

    #[test]
    fn test_download_error_subcategory() {
        assert_eq!(DownloadError::YtDlp("".into()).subcategory(), "ytdlp");
        assert_eq!(DownloadError::Ffmpeg("".into()).subcategory(), "ffmpeg");
        assert_eq!(DownloadError::Timeout("".into()).subcategory(), "timeout");
        assert_eq!(DownloadError::Proxy("".into()).subcategory(), "proxy");
        assert_eq!(DownloadError::DiskSpace("".into()).subcategory(), "disk_space");
        assert_eq!(DownloadError::Other("".into()).subcategory(), "other");
    }

    #[test]
    fn test_from_string() {
        let err: DownloadError = "test error".to_string().into();
        assert!(matches!(err, DownloadError::Other(_)));
        assert_eq!(err.message(), "test error");
    }
}
