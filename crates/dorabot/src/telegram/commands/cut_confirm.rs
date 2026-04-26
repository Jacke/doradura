//! Pre-cut confirmation step (GH #11).
//!
//! When the user types cut intervals (e.g., `00:10-00:45, 01:00-01:30`),
//! we used to immediately spawn `process_video_clip` with no chance to
//! review what was parsed. That was particularly painful with the
//! 60-second video-note cap: the user typed `01:00-02:00`, got back
//! a silently-truncated 60-second clip, and had to start over.
//!
//! Now we parse, stash the result in an in-memory cache, and show a
//! confirmation message:
//!
//! > 📋 Result: 65 sec (2 segments: 00:10–00:45, 01:00–01:30)
//! > [✅ Cut] [❌ Cancel]
//!
//! The cache is process-local — bot restart drops pending state and
//! the user simply re-enters their intervals. This avoids a migration
//! and Postgres pressure for what is intrinsically a transient UI step.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, ReplyParameters};

use crate::core::error::AppError;
use crate::i18n;
use crate::storage::SharedStorage;
use crate::storage::db::{self as db, DbPool, OutputKind};
use crate::telegram::Bot;

use super::CutSegment;

/// How long a pending cut survives without confirmation before we drop it.
/// Short window keeps stale state out of the way; user re-types if expired.
const PENDING_TTL: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub struct PendingCut {
    pub session: db::VideoClipSession,
    pub segments: Vec<CutSegment>,
    pub segments_text: String,
    pub speed: Option<f32>,
    pub created_at: Instant,
}

static PENDING_CUTS: LazyLock<Mutex<HashMap<i64, PendingCut>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Stash a parsed cut for later confirmation. Overwrites any prior pending
/// cut for the same user — typing fresh intervals abandons the previous
/// pending cut.
pub fn put(chat_id: i64, pending: PendingCut) {
    if let Ok(mut map) = PENDING_CUTS.lock() {
        cleanup_expired_locked(&mut map);
        map.insert(chat_id, pending);
    }
}

/// Take and remove the pending cut for `chat_id`. Returns `None` if there
/// is no pending cut or it has expired.
pub fn take(chat_id: i64) -> Option<PendingCut> {
    let mut map = PENDING_CUTS.lock().ok()?;
    cleanup_expired_locked(&mut map);
    map.remove(&chat_id)
}

/// Drop the pending cut for `chat_id` without consuming it (used on cancel).
pub fn drop_pending(chat_id: i64) {
    if let Ok(mut map) = PENDING_CUTS.lock() {
        map.remove(&chat_id);
    }
}

fn cleanup_expired_locked(map: &mut HashMap<i64, PendingCut>) {
    let now = Instant::now();
    map.retain(|_, pending| now.duration_since(pending.created_at) < PENDING_TTL);
}

/// Total wall-clock duration of the cut after speed adjustment, in seconds.
pub fn effective_duration_secs(segments: &[CutSegment], speed: Option<f32>) -> i64 {
    let raw: i64 = segments.iter().map(|s| (s.end_secs - s.start_secs).max(0)).sum();
    match speed {
        Some(spd) if spd > 0.0 => (raw as f32 / spd).ceil() as i64,
        _ => raw,
    }
}

/// Build the confirmation message body shown to the user. Includes total
/// duration, segment list, optional speed marker, and a video-note cap
/// warning when relevant.
pub fn build_preview_text(
    lang: &unic_langid::LanguageIdentifier,
    segments: &[CutSegment],
    segments_text: &str,
    speed: Option<f32>,
    output_kind: OutputKind,
) -> String {
    let total = effective_duration_secs(segments, speed);
    let count = segments.len();
    let count_label = if count == 1 {
        i18n::t(lang, "cut_confirm.segment_one")
    } else {
        i18n::t(lang, "cut_confirm.segment_many")
    };
    let speed_suffix = match speed {
        Some(spd) if (spd - 1.0).abs() > 0.01 => format!(" @ {:.2}x", spd),
        _ => String::new(),
    };
    let mut body = format!(
        "{}\n{}: {} sec ({} {}: {}){}",
        i18n::t(lang, "cut_confirm.title"),
        i18n::t(lang, "cut_confirm.result_label"),
        total,
        count,
        count_label,
        segments_text,
        speed_suffix,
    );
    if output_kind == OutputKind::VideoNote && total > 60 {
        body.push_str("\n\n");
        body.push_str(&i18n::t(lang, "cut_confirm.video_note_truncate_warning"));
    }
    body
}

/// Two-button keyboard: confirm or cancel.
pub fn build_keyboard(lang: &unic_langid::LanguageIdentifier) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![
        InlineKeyboardButton::callback(i18n::t(lang, "cut_confirm.button_confirm"), "cut_confirm:yes"),
        InlineKeyboardButton::callback(i18n::t(lang, "cut_confirm.button_cancel"), "cut_confirm:no"),
    ]])
}

/// Send the confirmation prompt and stash pending state. Called from the
/// text-intercept path after `parse_segments_spec` succeeds.
pub async fn send_confirmation(
    bot: &Bot,
    chat_id: ChatId,
    reply_to_message_id: Option<i32>,
    lang: &unic_langid::LanguageIdentifier,
    pending: PendingCut,
) -> ResponseResult<()> {
    let body = build_preview_text(
        lang,
        &pending.segments,
        &pending.segments_text,
        pending.speed,
        pending.session.output_kind,
    );
    let kbd = build_keyboard(lang);
    let mut req = bot.send_message(chat_id, body).reply_markup(kbd);
    if let Some(mid) = reply_to_message_id {
        req = req.reply_parameters(ReplyParameters::new(teloxide::types::MessageId(mid)));
    }
    req.await?;
    put(chat_id.0, pending);
    Ok(())
}

/// Handle the `cut_confirm:yes|no` callback. Returns `true` when the
/// callback was consumed by this handler (always, in practice — the
/// router only calls us for `CutConfirm` kind).
pub async fn handle_callback(
    bot: Bot,
    callback_id: teloxide::types::CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    data: &str,
    db_pool: std::sync::Arc<DbPool>,
    shared_storage: std::sync::Arc<SharedStorage>,
) -> Result<(), AppError> {
    let lang = i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;
    let action = data.strip_prefix("cut_confirm:").unwrap_or("");

    let _ = bot.answer_callback_query(callback_id).await;

    match action {
        "yes" => {
            let Some(pending) = take(chat_id.0) else {
                let _ = bot
                    .edit_message_text(chat_id, message_id, i18n::t(&lang, "cut_confirm.expired"))
                    .await;
                return Ok(());
            };
            // Drop the persisted session — same lifecycle the old direct-spawn
            // path had at line ~446 of commands/mod.rs.
            let _ = shared_storage.delete_video_clip_session_by_user(chat_id.0).await;
            let _ = bot
                .edit_message_text(chat_id, message_id, i18n::t(&lang, "cut_confirm.confirmed"))
                .await;
            // Spawn the actual cut work. Same call shape as the legacy direct path.
            let bot_clone = bot.clone();
            let db_clone = db_pool.clone();
            let storage_clone = shared_storage.clone();
            tokio::spawn(async move {
                if let Err(e) = super::process_video_clip(
                    bot_clone,
                    db_clone,
                    storage_clone,
                    chat_id,
                    pending.session,
                    pending.segments,
                    pending.segments_text,
                    pending.speed,
                )
                .await
                {
                    log::warn!("Failed to process video clip after confirm: {}", e);
                }
            });
        }
        "no" => {
            drop_pending(chat_id.0);
            let _ = shared_storage.delete_video_clip_session_by_user(chat_id.0).await;
            let _ = bot
                .edit_message_text(chat_id, message_id, i18n::t(&lang, "cut_confirm.cancelled"))
                .await;
        }
        _ => {
            log::warn!("Unknown cut_confirm action: {}", action);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_duration_no_speed() {
        let segs = vec![
            CutSegment {
                start_secs: 10,
                end_secs: 45,
            }, // 35
            CutSegment {
                start_secs: 60,
                end_secs: 90,
            }, // 30
        ];
        assert_eq!(effective_duration_secs(&segs, None), 65);
    }

    #[test]
    fn effective_duration_with_speed() {
        let segs = vec![CutSegment {
            start_secs: 0,
            end_secs: 60,
        }];
        // 60 / 2.0 = 30
        assert_eq!(effective_duration_secs(&segs, Some(2.0)), 30);
        // 60 / 1.5 = 40 (ceil)
        assert_eq!(effective_duration_secs(&segs, Some(1.5)), 40);
    }

    #[test]
    fn effective_duration_invalid_speed_falls_back_to_raw() {
        let segs = vec![CutSegment {
            start_secs: 0,
            end_secs: 30,
        }];
        assert_eq!(effective_duration_secs(&segs, Some(0.0)), 30);
        assert_eq!(effective_duration_secs(&segs, Some(-1.0)), 30);
    }

    #[test]
    fn put_and_take_roundtrip() {
        // Use a synthetic chat id unlikely to collide with other tests.
        let chat_id = -999_111;
        drop_pending(chat_id);
        let session = db::VideoClipSession {
            id: "test".into(),
            user_id: chat_id,
            source_download_id: 1,
            source_kind: db::SourceKind::Download,
            source_id: 1,
            original_url: String::new(),
            output_kind: OutputKind::Cut,
            created_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            subtitle_lang: None,
            custom_audio_file_id: None,
        };
        put(
            chat_id,
            PendingCut {
                session: session.clone(),
                segments: vec![CutSegment {
                    start_secs: 0,
                    end_secs: 30,
                }],
                segments_text: "00:00-00:30".into(),
                speed: None,
                created_at: Instant::now(),
            },
        );
        let got = take(chat_id).expect("pending cut should be present");
        assert_eq!(got.session.id, session.id);
        assert!(take(chat_id).is_none(), "second take should yield None");
    }
}
