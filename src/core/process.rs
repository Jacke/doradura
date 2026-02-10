//! Process execution utilities with timeout support
//!
//! Provides helpers for running external processes (ffmpeg, ffprobe, yt-dlp)
//! with configurable timeouts to prevent hung processes from blocking the pipeline.

use std::process::Output;
use std::time::Duration;
use tokio::process::Command;

use crate::core::error::AppError;

/// Default timeout for ffmpeg operations (2 minutes)
pub const FFMPEG_TIMEOUT: Duration = Duration::from_secs(120);

/// Default timeout for ffprobe metadata queries (30 seconds)
pub const FFPROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// Run an async Command with a timeout.
///
/// Returns the process Output on success, or an AppError on timeout/IO failure.
pub async fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> Result<Output, AppError> {
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(AppError::Io(e)),
        Err(_) => Err(AppError::Download(format!(
            "Process timed out after {}s",
            timeout.as_secs()
        ))),
    }
}
