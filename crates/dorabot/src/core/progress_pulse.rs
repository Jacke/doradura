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
use std::time::Duration;
use teloxide::prelude::*;
use teloxide::types::MessageId;
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
}
