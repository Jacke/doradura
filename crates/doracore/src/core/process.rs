//! Process execution utilities with timeout support
//!
//! Provides helpers for running external processes (ffmpeg, ffprobe, yt-dlp)
//! with configurable timeouts to prevent hung processes from blocking the pipeline.
//!
//! ## Why you should never inline `tokio::time::timeout(dur, cmd.output())`
//!
//! Without `cmd.kill_on_drop(true)`, when the timeout fires the future is dropped
//! but the subprocess **keeps running** until it finishes naturally. ffmpeg,
//! libreoffice, and yt-dlp can easily hold file handles, CPU, RAM, and worker
//! slots for many minutes past the nominal timeout — an invisible resource leak
//! in every error path. The helpers in this module always set `kill_on_drop`.

use std::process::Output;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::error::Elapsed;

use crate::core::error::AppError;
use crate::download::error::DownloadError;

/// Default timeout for ffmpeg operations (2 minutes)
pub const FFMPEG_TIMEOUT: Duration = Duration::from_secs(120);

/// Default timeout for ffprobe metadata queries (30 seconds)
pub const FFPROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// Run an async Command with a timeout. Returns the process `Output` on success,
/// or an `AppError` on timeout/IO failure.
///
/// Prefer this helper for simple error-propagation cases. If you need to
/// distinguish timeout from IO failure, or want to emit a user-facing message
/// on timeout, use [`run_with_timeout_raw`] instead.
pub async fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> Result<Output, AppError> {
    cmd.kill_on_drop(true);
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(AppError::Io(e)),
        Err(_) => Err(AppError::Download(DownloadError::Timeout(format!(
            "Process timed out after {}s",
            timeout.as_secs()
        )))),
    }
}

/// Run an async Command with a timeout, returning the raw nested result so the
/// caller can distinguish timeout from IO failure and attach custom logging /
/// user-facing messages to each case.
///
/// This is the replacement for `tokio::time::timeout(dur, cmd.output()).await`
/// — same shape, same pattern-match, plus `kill_on_drop(true)` is set so
/// subprocesses are reaped on timeout instead of leaking.
///
/// ```ignore
/// let output = match run_with_timeout_raw(&mut cmd, Duration::from_secs(60)).await {
///     Ok(Ok(o)) => o,
///     Ok(Err(e)) => return Err(AppError::from(e)),
///     Err(_) => {
///         bot.send_message(chat_id, "⏱ Timed out").await.ok();
///         return Ok(());
///     }
/// };
/// ```
pub async fn run_with_timeout_raw(cmd: &mut Command, timeout: Duration) -> Result<std::io::Result<Output>, Elapsed> {
    cmd.kill_on_drop(true);
    tokio::time::timeout(timeout, cmd.output()).await
}
