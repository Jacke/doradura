/// Structured error type for download operations.
///
/// Replaces the previous `AppError::Download(String)` with categorized variants
/// for better error handling, metrics, and debugging.
///
/// `Display` and `Error` are derived via `thiserror` — every variant prints its
/// inner message verbatim (`{0}`), which matches the original hand-rolled
/// `write!(f, "{}", msg)` output bytewise.
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    /// yt-dlp specific failures (binary not found, bad exit code, etc.)
    #[error("{0}")]
    YtDlp(String),
    /// FFmpeg processing failures (encoding, splitting, subtitle burn)
    #[error("{0}")]
    Ffmpeg(String),
    /// Expected file not found after processing
    #[error("{0}")]
    FileNotFound(String),
    /// Download or processing timed out
    #[error("{0}")]
    Timeout(String),
    /// Proxy configuration or connection error
    #[error("{0}")]
    Proxy(String),
    /// Insufficient disk space
    #[error("{0}")]
    DiskSpace(String),
    /// Failed to send file via Telegram API
    #[error("{0}")]
    SendFailed(String),
    /// Process execution failure (spawn, exit code)
    #[error("{0}")]
    Process(String),
    /// Instagram API specific failures (GraphQL errors, doc_id expiry, private accounts)
    #[error("{0}")]
    Instagram(String),
    /// Vlipsy API failures (search, clip fetch, download)
    #[error("{0}")]
    Vlipsy(String),
    /// Catch-all for uncategorized errors
    #[error("{0}")]
    Other(String),
}

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
            DownloadError::Vlipsy(_) => "vlipsy",
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
            | DownloadError::Vlipsy(msg)
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
