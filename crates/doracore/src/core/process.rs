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

/// Outcome of [`run_with_pulses`]. Distinguishes the three failure shapes the
/// caller usually wants to handle separately (timeout = friendly user message,
/// IO failure = retry path, success = inspect output).
pub enum PulseOutcome {
    Done(Output),
    Timeout,
    Io(std::io::Error),
}

/// Run a Command with periodic "still working" pulses for long-running ffmpeg
/// operations like /circle encode (GH #8). Caller supplies a closure that's
/// invoked every `pulse_every` with the elapsed wall-clock duration since the
/// subprocess started.
///
/// Wraps `tokio::process::Command::spawn` + `child.wait()` + a `tokio::time::interval`
/// in a `select!`, so the pulses fire while the subprocess runs and stop the
/// instant it exits. `kill_on_drop(true)` is set, matching the other helpers.
///
/// **Why pulse-based instead of parsing ffmpeg's `-progress pipe:1`**: the
/// machine-readable stream requires reshaping every call site's Command (extra
/// args, stdout-pipe routing, line parsing). Pulse-only doesn't give percent
/// but does prove "still alive" — the actual UX gap on silent /circle encodes.
/// Real-percent progress can layer on top of this helper later.
pub async fn run_with_pulses<F>(
    cmd: &mut Command,
    timeout: Duration,
    pulse_every: Duration,
    mut on_pulse: F,
) -> PulseOutcome
where
    F: FnMut(Duration) + Send,
{
    use tokio::io::AsyncReadExt;

    cmd.kill_on_drop(true);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let started = std::time::Instant::now();

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return PulseOutcome::Io(e),
    };

    let mut stdout_buf: Vec<u8> = Vec::new();
    let mut stderr_buf: Vec<u8> = Vec::new();
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();

    let mut ticker = tokio::time::interval(pulse_every);
    ticker.tick().await; // tokio::interval first tick fires immediately; drop it so the first user-visible pulse lands at +pulse_every, not t=0

    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            biased;
            _ = &mut deadline => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return PulseOutcome::Timeout;
            }
            _ = ticker.tick() => {
                on_pulse(started.elapsed());
            }
            wait_result = child.wait() => {
                if let Some(mut out) = stdout.take() {
                    let _ = out.read_to_end(&mut stdout_buf).await;
                }
                if let Some(mut err) = stderr.take() {
                    let _ = err.read_to_end(&mut stderr_buf).await;
                }
                return match wait_result {
                    Ok(status) => PulseOutcome::Done(Output { status, stdout: stdout_buf, stderr: stderr_buf }),
                    Err(e) => PulseOutcome::Io(e),
                };
            }
        }
    }
}
