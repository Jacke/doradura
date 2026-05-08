//! Long-video gate: interstitial panel for videos > 2h.
//!
//! When `send_preview` detects `metadata.duration > 7200`, it renders this
//! panel instead of the standard format keyboard. Goal: prevent users from
//! accidentally enqueuing a 4-hour 4K download that would OOM Railway,
//! exhaust disk, or hit Telegram's 2 GB-per-file cap.
//!
//! Panel offers four actions:
//!   - 🎵 MP3 audio (full)            → existing `dl:mp3:{url_id}` route
//!   - 📺 Continue with video anyway  → `long:ack:{url_id}` (sets ack flag, re-renders preview)
//!   - ✂️ Pick a time range          → `long:hint:{url_id}` (text instructions)
//!   - ❌ Cancel                      → `long:cancel:{url_id}` (deletes panel message)
//!
//! Phase 0 covers detection + UX gate only. Phase 1 will add an actual
//! chunked-download pipeline for full long videos; until then "Continue
//! anyway" routes through the existing single-file pipeline (still subject
//! to a 4h hard cap downstream).

use crate::storage::SharedStorage;
use crate::telegram::Bot;
use crate::telegram::cache;
use doracore::storage::DbPool;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, MessageId};

/// Threshold (seconds) above which we show the long-video gate. 2h.
pub const LONG_VIDEO_THRESHOLD_SECS: u32 = 7200;

/// Build the long-video panel keyboard.
///
/// `with_lyrics` flips the MP3 button between `dl:mp3:` and `dl:mp3+lyr:`
/// so the lyrics-toggle state from the previous render is preserved.
pub fn build_long_video_keyboard(
    url_id: &str,
    with_lyrics: bool,
    lang: &unic_langid::LanguageIdentifier,
) -> InlineKeyboardMarkup {
    let mp3_cb = if with_lyrics {
        format!("dl:mp3+lyr:{}", url_id)
    } else {
        format!("dl:mp3:{}", url_id)
    };

    let btn_mp3 = InlineKeyboardButton::callback(crate::i18n::t(lang, "long_video.btn_mp3"), mp3_cb);
    let btn_full = InlineKeyboardButton::callback(
        crate::i18n::t(lang, "long_video.btn_continue"),
        format!("long:ack:{}", url_id),
    );
    let btn_range = InlineKeyboardButton::callback(
        crate::i18n::t(lang, "long_video.btn_range"),
        format!("long:hint:{}", url_id),
    );
    let btn_cancel = InlineKeyboardButton::callback(
        crate::i18n::t(lang, "long_video.btn_cancel"),
        format!("long:cancel:{}", url_id),
    );

    InlineKeyboardMarkup::new(vec![vec![btn_mp3], vec![btn_full], vec![btn_range], vec![btn_cancel]])
}

/// Format the long-video panel body text.
pub fn format_panel_text(duration_secs: u32, lang: &unic_langid::LanguageIdentifier) -> String {
    let hours = duration_secs / 3600;
    let minutes = (duration_secs % 3600) / 60;
    let dur_str = if hours > 0 {
        format!("{}h {}min", hours, minutes)
    } else {
        format!("{}min", minutes)
    };
    let args = doracore::fluent_args!("duration" => dur_str);
    crate::i18n::t_args(lang, "long_video.panel", &args)
}

/// Dispatch `long:*` callbacks.
pub async fn handle_long_video_callback(
    bot: &Bot,
    callback_id: teloxide::types::CallbackQueryId,
    chat_id: ChatId,
    message_id: MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    let _ = bot.answer_callback_query(callback_id).await;
    let parts: Vec<&str> = data.split(':').collect();
    if parts.len() < 3 {
        return Ok(());
    }
    let action = parts[1];
    let url_id = parts[2];
    let lang = crate::i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;

    match action {
        "ack" => {
            // Resolve URL, set ack flag, re-render preview with the standard keyboard.
            let Some(url_str) =
                doracore::storage::cache::get_url(&db_pool, Some(shared_storage.as_ref()), url_id).await
            else {
                log::warn!("long:ack: url_id {} not found", url_id);
                return Ok(());
            };
            cache::store_long_ack(&url_str).await;

            let metadata = cache::PREVIEW_CACHE.get(&url_str).await;
            let Some(metadata) = metadata else {
                let _ = bot
                    .send_message(chat_id, crate::i18n::t(&lang, "long_video.cache_expired"))
                    .await;
                return Ok(());
            };

            let url = match url::Url::parse(&url_str) {
                Ok(u) => u,
                Err(e) => {
                    log::warn!("long:ack: invalid URL {}: {}", url_str, e);
                    return Ok(());
                }
            };

            // Re-render preview. Default format = mp4 (user explicitly chose
            // video). `send_preview` will see the long_ack flag and skip the
            // gate this time, falling through to the standard format keyboard.
            if let Err(e) = crate::telegram::preview::send_preview(
                bot,
                chat_id,
                &url,
                &metadata,
                "mp4",
                None,
                Some(message_id),
                db_pool,
                shared_storage,
                None,
            )
            .await
            {
                log::warn!("long:ack: send_preview re-render failed: {:?}", e);
            }
        }
        "hint" => {
            // Show instructions for typed time-range syntax. Inline picker
            // for pre-download time selection is Phase 2.
            let _ = bot
                .send_message(chat_id, crate::i18n::t(&lang, "long_video.range_hint"))
                .await;
        }
        "cancel" => {
            let _ = bot.delete_message(chat_id, message_id).await;
        }
        _ => {
            log::warn!("long: unknown action '{}'", action);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_is_two_hours() {
        assert_eq!(LONG_VIDEO_THRESHOLD_SECS, 7200);
    }

    #[test]
    fn format_panel_text_under_one_hour() {
        let lang = crate::i18n::lang_from_code("en-US");
        let s = format_panel_text(45 * 60, &lang);
        assert!(s.contains("45min"), "expected 45min in '{}'", s);
    }

    #[test]
    fn format_panel_text_multi_hours() {
        let lang = crate::i18n::lang_from_code("en-US");
        let s = format_panel_text(3 * 3600 + 47 * 60, &lang);
        assert!(s.contains("3h"));
        assert!(s.contains("47min"));
    }
}
