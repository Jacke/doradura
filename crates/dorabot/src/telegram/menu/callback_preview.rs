//! Preview-message callback handling — `pv:` prefix.
//!
//! Actions: `cancel`, `set`, `burn_subs`, `burn_subs_lang`, `audio`, `audio_lang`.
//! Extracted from `callback_router::handle_menu_callback` (this single
//! branch was 369 LOC and drove the function past the god-fn threshold).

use std::sync::Arc;

use teloxide::prelude::*;
use teloxide::types::MaybeInaccessibleMessage;
use url::Url;

use crate::storage::SharedStorage;
use crate::storage::cache;
use crate::storage::db::DbPool;
use crate::telegram::Bot;

use super::main_menu::{edit_main_menu, send_main_menu_as_new};

/// Entry point for `pv:*` callback queries.
///
/// Parses the `{prefix}:{action}:{rest}` triple and dispatches to the
/// matching action branch. All original inline logic is preserved
/// verbatim — this extraction is structural only.
#[allow(clippy::too_many_arguments)]
pub async fn handle_preview_callback(
    bot: &Bot,
    callback_id: teloxide::types::CallbackQueryId,
    message: Option<&MaybeInaccessibleMessage>,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    let parts: Vec<&str> = data.splitn(3, ':').collect();
    if parts.len() != 3 {
        return Ok(());
    }
    let action = parts[1];
    match action {
        "cancel" => {
            let _ = bot.answer_callback_query(callback_id.clone()).await;
            if let Err(e) = bot.delete_message(chat_id, message_id).await {
                log::warn!("Failed to delete preview message: {:?}", e);
            }
        }
        "set" => {
            let _ = bot.answer_callback_query(callback_id.clone()).await;
            let url_id = parts[2];
            let preview_msg_id = message_id;

            let has_photo = message
                .and_then(|m| match m {
                    MaybeInaccessibleMessage::Regular(msg) => msg.photo(),
                    _ => None,
                })
                .is_some();

            if has_photo {
                if let Err(e) = bot.delete_message(chat_id, message_id).await {
                    log::warn!("Failed to delete preview message: {:?}", e);
                }
                send_main_menu_as_new(
                    bot,
                    chat_id,
                    Arc::clone(&db_pool),
                    Arc::clone(&shared_storage),
                    Some(url_id),
                    Some(preview_msg_id),
                )
                .await?;
            } else {
                edit_main_menu(
                    bot,
                    chat_id,
                    message_id,
                    Arc::clone(&db_pool),
                    Arc::clone(&shared_storage),
                    Some(url_id),
                    Some(preview_msg_id),
                )
                .await?;
            }
        }
        "burn_subs" => {
            let _ = bot.answer_callback_query(callback_id.clone()).await;
            let url_id = parts[2];
            let lang_options = vec![
                vec![
                    crate::telegram::cb("en".to_string(), format!("pv:burn_subs_lang:en:{}", url_id)),
                    crate::telegram::cb("ru".to_string(), format!("pv:burn_subs_lang:ru:{}", url_id)),
                    crate::telegram::cb("uk".to_string(), format!("pv:burn_subs_lang:uk:{}", url_id)),
                    crate::telegram::cb("es".to_string(), format!("pv:burn_subs_lang:es:{}", url_id)),
                    crate::telegram::cb("pt".to_string(), format!("pv:burn_subs_lang:pt:{}", url_id)),
                ],
                vec![
                    crate::telegram::cb("ar".to_string(), format!("pv:burn_subs_lang:ar:{}", url_id)),
                    crate::telegram::cb("fa".to_string(), format!("pv:burn_subs_lang:fa:{}", url_id)),
                    crate::telegram::cb("fr".to_string(), format!("pv:burn_subs_lang:fr:{}", url_id)),
                    crate::telegram::cb("de".to_string(), format!("pv:burn_subs_lang:de:{}", url_id)),
                    crate::telegram::cb("hi".to_string(), format!("pv:burn_subs_lang:hi:{}", url_id)),
                ],
                vec![crate::telegram::cb(
                    "❌ No subs".to_string(),
                    format!("pv:burn_subs_lang:none:{}", url_id),
                )],
            ];
            let keyboard = teloxide::types::InlineKeyboardMarkup::new(lang_options);
            if let Err(e) = bot
                .edit_message_reply_markup(chat_id, message_id)
                .reply_markup(keyboard)
                .await
            {
                log::warn!("Failed to edit preview keyboard for burn_subs picker: {:?}", e);
            }
        }
        "burn_subs_lang" => {
            let _ = bot.answer_callback_query(callback_id.clone()).await;
            let rest = parts[2];
            let (lang_code, url_id) = match rest.split_once(':') {
                Some((l, u)) => (l.to_string(), u.to_string()),
                None => return Ok(()),
            };

            const VALID_SUB_LANGS: &[&str] = &[
                "en", "ru", "uk", "es", "pt", "de", "fr", "ar", "fa", "hi", "ja", "ko", "zh", "it", "nl", "pl", "tr",
                "none",
            ];
            if !VALID_SUB_LANGS.contains(&lang_code.as_str()) {
                log::warn!(
                    "Rejected invalid sub lang value from user {}: {:?}",
                    chat_id.0,
                    lang_code
                );
                return Ok(());
            }

            let url_str = match cache::get_url(&db_pool, Some(shared_storage.as_ref()), &url_id).await {
                Some(u) => u,
                None => {
                    bot.send_message(chat_id, "❌ Link expired, please send the URL again")
                        .await?;
                    return Ok(());
                }
            };

            if lang_code == "none" {
                let _ = shared_storage
                    .set_preview_burn_sub_lang(chat_id.0, &url_str, None, 3600)
                    .await;
            } else {
                let _ = shared_storage
                    .set_preview_burn_sub_lang(chat_id.0, &url_str, Some(&lang_code), 3600)
                    .await;
            }

            let url = match Url::parse(&url_str) {
                Ok(u) => u,
                Err(e) => {
                    log::error!("Failed to parse URL from cache: {}", e);
                    let _ = bot.send_message(chat_id, "❌ Error: invalid link").await;
                    return Ok(());
                }
            };

            let current_format = shared_storage
                .get_user_download_format(chat_id.0)
                .await
                .unwrap_or_else(|_| "mp4".to_string());
            let video_quality = shared_storage.get_user_video_quality(chat_id.0).await.ok();
            // Experimental features graduated to main workflow

            match crate::telegram::preview::get_preview_metadata(&url, Some(&current_format), video_quality.as_deref())
                .await
            {
                Ok(metadata) => {
                    let preview_context = shared_storage
                        .get_preview_context(chat_id.0, url.as_str())
                        .await
                        .ok()
                        .flatten();
                    let time_range = preview_context.as_ref().and_then(|context| context.time_range.clone());
                    match crate::telegram::preview::update_preview_message(
                        bot,
                        chat_id,
                        message_id,
                        &url,
                        &metadata,
                        &current_format,
                        video_quality.as_deref(),
                        Arc::clone(&db_pool),
                        Arc::clone(&shared_storage),
                        time_range.as_ref(),
                    )
                    .await
                    {
                        Ok(_) => {}
                        Err(e) => {
                            log::error!("Failed to update preview after burn_subs_lang selection: {:?}", e);
                            let _ = bot
                                .send_message(chat_id, "Failed to update preview. Please send the link again.")
                                .await;
                        }
                    }
                }
                Err(e) => {
                    log::error!(
                        "Failed to refresh preview metadata after burn_subs_lang selection: {:?}",
                        e
                    );
                    let _ = bot
                        .send_message(chat_id, "⏰ Preview expired, please send the link again")
                        .await;
                }
            }
        }
        "audio" => {
            let _ = bot.answer_callback_query(callback_id.clone()).await;
            let url_id = parts[2];

            // Get URL from cache
            let url_str = match cache::get_url(&db_pool, Some(shared_storage.as_ref()), url_id).await {
                Some(u) => u,
                None => {
                    bot.send_message(chat_id, "❌ Link expired, please send the URL again")
                        .await?;
                    return Ok(());
                }
            };

            // Get audio tracks from preview cache
            let audio_tracks = crate::telegram::cache::PREVIEW_CACHE
                .get(&url_str)
                .await
                .and_then(|meta| meta.audio_tracks);

            let mut rows: Vec<Vec<teloxide::types::InlineKeyboardButton>> = Vec::new();
            if let Some(tracks) = audio_tracks {
                let mut row = Vec::new();
                for track in &tracks {
                    let label = track
                        .display_name
                        .as_deref()
                        .map(|name| format!("{} ({})", track.language, name))
                        .unwrap_or_else(|| track.language.clone());
                    row.push(crate::telegram::cb(
                        label,
                        format!("pv:audio_lang:{}:{}", track.language, url_id),
                    ));
                    if row.len() == 3 {
                        rows.push(std::mem::take(&mut row));
                    }
                }
                if !row.is_empty() {
                    rows.push(row);
                }
            }
            // "Original (no preference)" reset button
            rows.push(vec![crate::telegram::cb(
                "🔊 Original".to_string(),
                format!("pv:audio_lang:none:{}", url_id),
            )]);

            let keyboard = teloxide::types::InlineKeyboardMarkup::new(rows);
            if let Err(e) = bot
                .edit_message_reply_markup(chat_id, message_id)
                .reply_markup(keyboard)
                .await
            {
                log::warn!("Failed to edit preview keyboard for audio picker: {:?}", e);
            }
        }
        "audio_lang" => {
            let _ = bot.answer_callback_query(callback_id.clone()).await;
            let rest = parts[2];
            let (lang_code, url_id) = match rest.split_once(':') {
                Some((l, u)) => (l.to_string(), u.to_string()),
                None => return Ok(()),
            };

            let url_str = match cache::get_url(&db_pool, Some(shared_storage.as_ref()), &url_id).await {
                Some(u) => u,
                None => {
                    bot.send_message(chat_id, "❌ Link expired, please send the URL again")
                        .await?;
                    return Ok(());
                }
            };

            if lang_code == "none" {
                log::info!("🔊 Clearing audio_lang for user {} url {}", chat_id.0, url_str);
                if let Err(e) = shared_storage
                    .set_preview_audio_lang(chat_id.0, &url_str, None, 3600)
                    .await
                {
                    log::error!("Failed to clear audio_lang: {:?}", e);
                    let _ = bot
                        .send_message(chat_id, "❌ Failed to save audio language selection")
                        .await;
                    return Ok(());
                }
            } else {
                log::info!(
                    "🔊 Setting audio_lang='{}' for user {} url {}",
                    lang_code,
                    chat_id.0,
                    url_str
                );
                if let Err(e) = shared_storage
                    .set_preview_audio_lang(chat_id.0, &url_str, Some(&lang_code), 3600)
                    .await
                {
                    log::error!("Failed to set audio_lang: {:?}", e);
                    let _ = bot
                        .send_message(chat_id, "❌ Failed to save audio language selection")
                        .await;
                    return Ok(());
                }
            }

            // Rebuild the preview keyboard
            let url = match Url::parse(&url_str) {
                Ok(u) => u,
                Err(e) => {
                    log::error!("Failed to parse URL from cache: {}", e);
                    let _ = bot.send_message(chat_id, "❌ Error: invalid link").await;
                    return Ok(());
                }
            };

            let current_format = shared_storage
                .get_user_download_format(chat_id.0)
                .await
                .unwrap_or_else(|_| "mp4".to_string());
            let video_quality = shared_storage.get_user_video_quality(chat_id.0).await.ok();
            // Experimental features graduated to main workflow

            match crate::telegram::preview::get_preview_metadata(&url, Some(&current_format), video_quality.as_deref())
                .await
            {
                Ok(metadata) => {
                    let preview_context = shared_storage
                        .get_preview_context(chat_id.0, url.as_str())
                        .await
                        .ok()
                        .flatten();
                    let time_range = preview_context.as_ref().and_then(|ctx| ctx.time_range.clone());
                    if let Err(e) = crate::telegram::preview::update_preview_message(
                        bot,
                        chat_id,
                        message_id,
                        &url,
                        &metadata,
                        &current_format,
                        video_quality.as_deref(),
                        Arc::clone(&db_pool),
                        Arc::clone(&shared_storage),
                        time_range.as_ref(),
                    )
                    .await
                    {
                        log::error!("Failed to update preview after audio_lang selection: {:?}", e);
                        let _ = bot
                            .send_message(chat_id, "Failed to update preview. Please send the link again.")
                            .await;
                    }
                }
                Err(e) => {
                    log::error!("Failed to refresh preview metadata after audio_lang selection: {:?}", e);
                    let _ = bot
                        .send_message(chat_id, "⏰ Preview expired, please send the link again")
                        .await;
                }
            }
        }
        _ => {
            bot.answer_callback_query(callback_id).text("Unknown action").await?;
        }
    }
    Ok(())
}
