//! Pulse-aware ffmpeg runner with a Telegram message-edit watcher.
//!
//! Wraps the proven channel + spawned-watcher + `run_with_pulses` pattern
//! used in `circle/mod.rs`, `circle/audio_cut.rs`, `download/pipeline.rs`,
//! and `menu/audio_effects.rs` into one helper. Three reasons:
//!
//!   1. **Coverage** — circle's two retry paths (av_retry + video-only retry)
//!      and `voice_effects` ran with [`run_with_timeout_raw`], so on retries
//!      the user's status message froze for up to 10 minutes — the original
//!      "circle progress" UX gap GH #8 was opened to close.
//!   2. **DRY** — six near-identical 30-line blocks in callers.
//!   3. **Future percent-progress** — when we want to layer ffmpeg
//!      `-progress pipe:1` parsing on top, one place changes, not seven.
//!
//! The watcher edits `status_msg_id` every `pulse_every` (3 s by default)
//! with `"{label}… {Ns} elapsed"`. Edits are best-effort: a deleted message
//! or a Telegram rate-limit just drops the pulse silently. The watcher task
//! exits as soon as the channel sender is dropped — i.e. the instant
//! `run_with_pulses` returns.

use crate::telegram::Bot;
use doracore::core::process::{PulseOutcome, run_with_pulses};
use std::process::{Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use teloxide::prelude::*;
use teloxide::types::MessageId;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

/// Format the watcher's status text. Pure function — unit-testable, kept
/// out of the async runner so we can lock the wording in tests.
pub fn format_pulse_text(label: &str, elapsed: Duration) -> String {
    format!("{}… {}s elapsed", label, elapsed.as_secs())
}

/// Run an ffmpeg subprocess, surfacing "still alive" pulses to a Telegram
/// status message. Default pulse cadence is 3 s — matches existing callers.
///
/// `label` is the leading icon + verb (e.g. `"🎬 Encoding circle"` or
/// `"🎤 Applying voice effect"`). The watcher renders `{label}… {Ns} elapsed`.
pub async fn run_ffmpeg_with_progress(
    bot: &Bot,
    chat_id: ChatId,
    status_msg_id: MessageId,
    cmd: &mut Command,
    timeout: Duration,
    label: &'static str,
) -> PulseOutcome {
    run_ffmpeg_with_progress_every(bot, chat_id, status_msg_id, cmd, timeout, label, Duration::from_secs(3)).await
}

/// Like [`run_ffmpeg_with_progress`] but with a custom pulse cadence. Test
/// helper / for the rare caller that wants tighter or looser pulses.
pub async fn run_ffmpeg_with_progress_every(
    bot: &Bot,
    chat_id: ChatId,
    status_msg_id: MessageId,
    cmd: &mut Command,
    timeout: Duration,
    label: &'static str,
    pulse_every: Duration,
) -> PulseOutcome {
    let (pulse_tx, mut pulse_rx) = tokio::sync::mpsc::unbounded_channel::<Duration>();
    let watcher_bot = bot.clone();
    let watcher = tokio::spawn(async move {
        while let Some(elapsed) = pulse_rx.recv().await {
            let body = format_pulse_text(label, elapsed);
            let _ = watcher_bot.edit_message_text(chat_id, status_msg_id, body).await;
        }
    });

    let outcome = run_with_pulses(cmd, timeout, pulse_every, move |elapsed| {
        let _ = pulse_tx.send(elapsed);
    })
    .await;

    // Sender drops at end of `run_with_pulses` scope, so the watcher's recv()
    // returns None — let it drain and exit cleanly.
    let _ = watcher.await;

    outcome
}

/// Build a 10-segment Unicode block bar + percent + secs/total label.
///
/// Output shape: `"{label}… ▰▰▰▰▰▱▱▱▱▱ 50% · 12s/24s"`. Pure function —
/// unit-testable, locked by tests so future tweaks can't drift the wording.
pub fn format_progress_bar_text(label: &str, _elapsed: Duration, out_secs: u64, total_secs: u64) -> String {
    let total = total_secs.max(1);
    // Floor at 99 to avoid the "100% — still encoding" UX glitch when ffmpeg
    // briefly reports out_time_us ≥ total during the trailing-frame metadata
    // flush. We only render 100% when the process actually exits (watcher
    // sees no more pulses).
    let pct_raw = (out_secs as f64 / total as f64) * 100.0;
    let pct_display = pct_raw.floor().clamp(0.0, 99.0) as u8;
    let filled = ((pct_display as f64 / 100.0) * 10.0).round() as usize;
    let filled = filled.min(10);
    let bar: String = "▰".repeat(filled) + &"▱".repeat(10 - filled);
    format!("{}… {} {}% · {}s/{}s", label, bar, pct_display, out_secs, total)
}

/// Run an ffmpeg subprocess and stream a **real percent-progress bar** to a
/// Telegram status message. Parses ffmpeg's `-progress pipe:1` stdout output
/// (`out_time_us=...` lines per second) and renders a 10-block bar with
/// `% · Ns/Ms` against the known total output duration.
///
/// **Caller responsibilities:**
///   1. Add `.arg("-progress").arg("pipe:1")` to the command BEFORE the
///      output path (ffmpeg requires `-progress` before the output spec).
///   2. Pass the EFFECTIVE output duration in `total_secs` — i.e. after
///      `setpts/atempo` for speed-adjusted clips. ffmpeg's `out_time_us`
///      tracks the output file, not the input.
///
/// Falls back to elapsed-only display until the first `out_time_us` line
/// arrives (no flicker, just initial "0% · 0s/Ns").
pub async fn run_ffmpeg_with_progress_bar(
    bot: &Bot,
    chat_id: ChatId,
    status_msg_id: MessageId,
    cmd: &mut Command,
    timeout: Duration,
    label: &'static str,
    total_secs: u64,
) -> PulseOutcome {
    cmd.kill_on_drop(true);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let started = std::time::Instant::now();
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return PulseOutcome::Io(e),
    };

    let stdout = child.stdout.take();
    let mut stderr = child.stderr.take();

    // Atomic latch — parser writes the latest `out_time_us`, ticker reads it
    // every 3 s and pushes to the watcher. Decouples ffmpeg's ~1 s cadence
    // from Telegram edit rate-limit.
    let progress_us = Arc::new(AtomicU64::new(0));
    let parser_progress = Arc::clone(&progress_us);
    let parser = stdout.map(|stdout| {
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if let Some(value) = line.strip_prefix("out_time_us=")
                    && let Ok(us) = value.trim().parse::<u64>()
                {
                    parser_progress.store(us, Ordering::Relaxed);
                }
            }
        })
    });

    let (pulse_tx, mut pulse_rx) = tokio::sync::mpsc::unbounded_channel::<(Duration, u64)>();
    let watcher_bot = bot.clone();
    let watcher = tokio::spawn(async move {
        while let Some((elapsed, out_us)) = pulse_rx.recv().await {
            let out_secs = out_us / 1_000_000;
            let body = format_progress_bar_text(label, elapsed, out_secs, total_secs);
            let _ = watcher_bot.edit_message_text(chat_id, status_msg_id, body).await;
        }
    });

    let mut ticker = tokio::time::interval(Duration::from_secs(3));
    ticker.tick().await; // first tick fires immediately — drop it so initial pulse lands at +3s, not t=0

    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);

    let outcome = loop {
        tokio::select! {
            biased;
            _ = &mut deadline => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                break PulseOutcome::Timeout;
            }
            _ = ticker.tick() => {
                let elapsed = started.elapsed();
                let us = progress_us.load(Ordering::Relaxed);
                let _ = pulse_tx.send((elapsed, us));
            }
            wait_result = child.wait() => {
                let mut stderr_buf = Vec::new();
                if let Some(mut err) = stderr.take() {
                    let _ = err.read_to_end(&mut stderr_buf).await;
                }
                break match wait_result {
                    Ok(status) => PulseOutcome::Done(Output {
                        status,
                        stdout: Vec::new(),
                        stderr: stderr_buf,
                    }),
                    Err(e) => PulseOutcome::Io(e),
                };
            }
        }
    };

    drop(pulse_tx);
    if let Some(p) = parser {
        let _ = p.await;
    }
    let _ = watcher.await;

    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulse_text_includes_label_and_seconds() {
        let s = format_pulse_text("🎬 Encoding circle", Duration::from_secs(7));
        assert_eq!(s, "🎬 Encoding circle… 7s elapsed");
    }

    #[test]
    fn pulse_text_zero_seconds_is_explicit() {
        // First pulse never fires at t=0 — the interval first-tick is dropped
        // — but if a caller manually invokes the formatter with 0s it should
        // still render readably.
        let s = format_pulse_text("✂️ Encoding cut", Duration::ZERO);
        assert_eq!(s, "✂️ Encoding cut… 0s elapsed");
    }

    #[test]
    fn pulse_text_truncates_subsecond_to_floor() {
        // Duration::as_secs() truncates — caller relies on that for stable
        // updates rather than churn.
        let s = format_pulse_text("🎤 Voice effect", Duration::from_millis(2999));
        assert_eq!(s, "🎤 Voice effect… 2s elapsed");
    }

    #[test]
    fn progress_bar_renders_half_filled_at_50_percent() {
        let s = format_progress_bar_text("🎬 Encoding circle", Duration::from_secs(12), 30, 60);
        assert!(s.contains("▰▰▰▰▰▱▱▱▱▱"), "expected half bar in '{}'", s);
        assert!(s.contains("50%"), "expected 50% in '{}'", s);
        assert!(s.contains("30s/60s"), "expected 30s/60s in '{}'", s);
    }

    #[test]
    fn progress_bar_zero_secs_is_empty_bar() {
        let s = format_progress_bar_text("🎬 Encoding circle", Duration::ZERO, 0, 60);
        assert!(s.contains("▱▱▱▱▱▱▱▱▱▱"), "expected empty bar in '{}'", s);
        assert!(s.contains("0%"));
    }

    #[test]
    fn progress_bar_caps_at_99_percent_to_avoid_premature_100() {
        // ffmpeg can briefly report out_secs > total when output container
        // adds trailing-frame metadata. Clamp at 99% so we never lie about
        // "100% done" before the process actually exits.
        let s = format_progress_bar_text("🎬", Duration::ZERO, 65, 60);
        assert!(!s.contains("100%"), "should clamp under 100%, got: '{}'", s);
        assert!(s.contains("99%"));
    }

    #[test]
    fn progress_bar_divides_by_one_when_total_zero() {
        // Defensive — if caller mis-passes total=0, don't divide by zero.
        let s = format_progress_bar_text("🎬", Duration::ZERO, 5, 0);
        assert!(s.contains("99%"), "got '{}'", s);
    }
}
