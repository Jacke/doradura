use crate::core::history::handle_history_callback;
use crate::core::rate_limiter::RateLimiter;
use crate::download::queue::DownloadQueue;
use crate::downsub::DownsubGateway;
use crate::extension::ExtensionRegistry;
use crate::i18n;
use crate::storage::db::DbPool;
use crate::storage::SharedStorage;
use crate::storage::SubtitleCache;
use crate::telegram::admin;
use crate::telegram::Bot;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::RequestError;

use super::audio_effects::{handle_audio_cut_callback, handle_audio_effects_callback};
use super::callback_admin;
use super::callback_kind::CallbackKind;
use super::callback_settings;
use super::helpers::start_download_from_preview;
use super::lyrics::handle_lyrics_callback;
use super::settings::show_download_type_menu;

/// Handles callback queries from the menu inline keyboards.
///
/// Processes button presses, updates user settings, or switches between menus.
pub async fn handle_menu_callback(
    bot: Bot,
    q: CallbackQuery,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    download_queue: Arc<DownloadQueue>,
    rate_limiter: Arc<RateLimiter>,
    extension_registry: Arc<ExtensionRegistry>,
    downsub_gateway: Arc<DownsubGateway>,
    subtitle_cache: Arc<SubtitleCache>,
) -> ResponseResult<()> {
    let callback_id = q.id.clone();
    let data_clone = q.data.clone();
    let message_clone = q.message.clone();

    if let Some(data) = q.data {
        let chat_id = q.message.as_ref().map(|m| m.chat().id);
        let message_id = q.message.as_ref().map(|m| m.id());

        if let (Some(chat_id), Some(message_id)) = (chat_id, message_id) {
            let kind = CallbackKind::parse(&data);

            // Blocked user check (skip for admins and admin callbacks)
            let is_admin_callback = kind.is_some_and(|k| matches!(k, CallbackKind::Au | CallbackKind::Admin));
            if !is_admin_callback {
                let caller_id = i64::try_from(q.from.id.0).unwrap_or(0);
                if !admin::is_admin(caller_id) {
                    match shared_storage.get_user(caller_id).await {
                        Ok(Some(user)) if user.is_blocked => {
                            let _ = bot.answer_callback_query(callback_id).await;
                            return Ok(());
                        }
                        Ok(_) => {}
                        Err(e) => {
                            log::error!("Failed to check blocked status for callback {}: {}", caller_id, e);
                            let _ = bot.answer_callback_query(callback_id).await;
                            return Ok(());
                        }
                    }
                }
            }

            let lang = i18n::user_lang_from_storage(&shared_storage, chat_id.0).await;

            // Lyrics callbacks
            if data.starts_with("lyr:") {
                let lyr_query = CallbackQuery {
                    id: callback_id.clone(),
                    from: q.from.clone(),
                    message: message_clone.clone(),
                    inline_message_id: q.inline_message_id.clone(),
                    chat_instance: q.chat_instance.clone(),
                    data: data_clone.clone(),
                    game_short_name: q.game_short_name.clone(),
                };
                if let Err(e) = handle_lyrics_callback(bot.clone(), lyr_query, Arc::clone(&shared_storage)).await {
                    log::error!("Lyrics callback error: {}", e);
                }
                return Ok(());
            }

            // Handle audio cut/effects callbacks first
            if data.starts_with("ac:") {
                let ac_query = CallbackQuery {
                    id: callback_id.clone(),
                    from: q.from.clone(),
                    message: message_clone,
                    inline_message_id: q.inline_message_id.clone(),
                    chat_instance: q.chat_instance.clone(),
                    data: data_clone,
                    game_short_name: q.game_short_name.clone(),
                };
                if let Err(e) = handle_audio_cut_callback(bot.clone(), ac_query, Arc::clone(&shared_storage)).await {
                    log::error!("Audio cut callback error: {}", e);
                }
                return Ok(());
            }
            if data.starts_with("ae:") {
                let ae_query = CallbackQuery {
                    id: callback_id.clone(),
                    from: q.from.clone(),
                    message: message_clone,
                    inline_message_id: q.inline_message_id.clone(),
                    chat_instance: q.chat_instance.clone(),
                    data: data_clone,
                    game_short_name: q.game_short_name.clone(),
                };
                if let Err(e) = handle_audio_effects_callback(bot.clone(), ae_query, Arc::clone(&shared_storage)).await
                {
                    log::error!("Audio effects callback error: {}", e);
                }
                return Ok(());
            }

            // --- Delegated: admin-related callbacks (analytics:, metrics:, au:, admin:) ---
            if kind.is_some_and(CallbackKind::is_admin_group)
                && callback_admin::handle_admin_callback(
                    &bot,
                    &callback_id,
                    chat_id,
                    message_id,
                    &data,
                    &q.from,
                    Arc::clone(&db_pool),
                    Arc::clone(&shared_storage),
                )
                .await?
            {
                return Ok(());
            }

            // --- Delegated: settings-related callbacks ---
            if kind.is_some_and(CallbackKind::is_settings_group)
                && callback_settings::handle_settings_callback(
                    &bot,
                    &callback_id,
                    chat_id,
                    message_id,
                    &data,
                    &q.from,
                    q.message.as_ref(),
                    &lang,
                    Arc::clone(&db_pool),
                    Arc::clone(&shared_storage),
                    Arc::clone(&extension_registry),
                )
                .await?
            {
                return Ok(());
            }

            // --- Remaining callbacks handled inline ---

            if data.starts_with("ct:") {
                // Carousel toggle: ct:{index}:{url_id}:{mask} or ct:all:{url_id}:{mask}
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                let parts: Vec<&str> = data.splitn(4, ':').collect();
                if parts.len() == 4 {
                    let url_id = parts[2];
                    if let Ok(mask) = parts[3].parse::<u32>() {
                        let carousel_count = q
                            .message
                            .as_ref()
                            .and_then(|m| match m {
                                teloxide::types::MaybeInaccessibleMessage::Regular(msg) => msg.reply_markup(),
                                _ => None,
                            })
                            .map(|kb| {
                                kb.inline_keyboard
                                    .iter()
                                    .flat_map(|row| row.iter())
                                    .filter(|btn| {
                                        matches!(&btn.kind, teloxide::types::InlineKeyboardButtonKind::CallbackData(d) if d.starts_with("ct:") && d.chars().nth(3).is_some_and(|c| c.is_ascii_digit()))
                                    })
                                    .count() as u8
                            })
                            .unwrap_or(0);

                        if carousel_count > 0 {
                            let new_keyboard =
                                crate::telegram::preview::create_carousel_keyboard(carousel_count, mask, url_id);
                            let _ = bot
                                .edit_message_reply_markup(chat_id, message_id)
                                .reply_markup(new_keyboard)
                                .await;
                        }
                    }
                }
            } else if data.starts_with("ig:sub:") {
                let username = data.strip_prefix("ig:sub:").unwrap_or("");
                if !username.is_empty() {
                    let registry = std::sync::Arc::new(crate::watcher::WatcherRegistry::default_registry());
                    crate::telegram::subscriptions::show_subscribe_confirm(
                        &bot,
                        chat_id,
                        username,
                        &db_pool,
                        &shared_storage,
                        &registry,
                    )
                    .await;
                }
                return Ok(());
            } else if data.starts_with("ig:") {
                if let Err(e) = crate::telegram::instagram::handle_instagram_callback(
                    &bot,
                    &callback_id,
                    chat_id,
                    &data,
                    Arc::clone(&db_pool),
                    Arc::clone(&shared_storage),
                )
                .await
                {
                    log::error!("Instagram callback error: {}", e);
                }
                return Ok(());
            } else if data.starts_with("cw:") {
                let registry = std::sync::Arc::new(crate::watcher::WatcherRegistry::default_registry());
                crate::telegram::subscriptions::handle_subscription_callback(
                    &bot,
                    &callback_id,
                    chat_id,
                    message_id,
                    &data,
                    Arc::clone(&db_pool),
                    Arc::clone(&shared_storage),
                    &registry,
                )
                .await;
                return Ok(());
            } else if data.starts_with("format:") {
                // Format: format:mp3 or format:mp3:preview:url_id or format:mp3:preview:url_id:preview_msg_id
                let parts: Vec<&str> = data.split(':').collect();
                let format = parts[1];
                let is_from_preview = parts.len() >= 4 && parts[2] == "preview";
                let url_id = if is_from_preview { Some(parts[3]) } else { None };
                let preview_msg_id = if is_from_preview && parts.len() >= 5 {
                    parts[4].parse::<i32>().ok().map(teloxide::types::MessageId)
                } else {
                    None
                };
                if !is_from_preview {
                    let _ = bot.answer_callback_query(callback_id.clone()).await;
                }

                shared_storage
                    .set_user_download_format(chat_id.0, format)
                    .await
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                if is_from_preview {
                    if let Some(id) = url_id {
                        start_download_from_preview(
                            &bot,
                            &callback_id,
                            chat_id,
                            message_id,
                            preview_msg_id,
                            id,
                            format,
                            None,
                            Arc::clone(&db_pool),
                            Arc::clone(&shared_storage),
                            Arc::clone(&download_queue),
                            Arc::clone(&rate_limiter),
                        )
                        .await?;
                    }
                } else {
                    show_download_type_menu(
                        &bot,
                        chat_id,
                        message_id,
                        Arc::clone(&db_pool),
                        Arc::clone(&shared_storage),
                        None,
                        None,
                    )
                    .await?;
                }
            } else if data.starts_with("dl:tl:") {
                // Lyrics toggle: flip mp3 buttons between dl:mp3: and dl:mp3+lyr:
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                let parts: Vec<&str> = data.split(':').collect();
                if parts.len() >= 3 {
                    let url_id = parts[2];
                    if let Some(teloxide::types::MaybeInaccessibleMessage::Regular(regular_msg)) = q.message.as_ref() {
                        if let Some(keyboard) = regular_msg.reply_markup() {
                            let mut new_buttons = keyboard.inline_keyboard.clone();

                            let currently_on = new_buttons.iter().flatten().any(|btn| {
                                matches!(&btn.kind,
                                    teloxide::types::InlineKeyboardButtonKind::CallbackData(d)
                                    if d.starts_with("dl:mp3+lyr:"))
                            });

                            for row in &mut new_buttons {
                                for button in row {
                                    if let teloxide::types::InlineKeyboardButtonKind::CallbackData(ref mut cb) =
                                        button.kind
                                    {
                                        if currently_on {
                                            if let Some(rest) = cb.strip_prefix("dl:mp3+lyr:") {
                                                *cb = format!("dl:mp3:{}", rest);
                                            }
                                        } else if cb.starts_with("dl:mp3:") && !cb.starts_with("dl:mp3+lyr:") {
                                            let rest = cb.trim_start_matches("dl:mp3:");
                                            *cb = format!("dl:mp3+lyr:{}", rest);
                                        }
                                    }
                                }
                            }

                            for row in &mut new_buttons {
                                for button in row {
                                    if let teloxide::types::InlineKeyboardButtonKind::CallbackData(ref cb) = button.kind
                                    {
                                        if cb == &format!("dl:tl:{}", url_id) {
                                            button.text = if currently_on {
                                                "☐ 📝 Lyrics".to_string()
                                            } else {
                                                "☑ 📝 Lyrics".to_string()
                                            };
                                        }
                                    }
                                }
                            }

                            let new_keyboard = teloxide::types::InlineKeyboardMarkup::new(new_buttons);
                            let _ = crate::telegram::styled::edit_message_reply_markup_styled(
                                &bot,
                                chat_id,
                                message_id,
                                &new_keyboard,
                            )
                            .await;
                            log::info!(
                                "Lyrics toggle: {} → {} for user {}",
                                if currently_on { "ON" } else { "OFF" },
                                if currently_on { "OFF" } else { "ON" },
                                chat_id.0
                            );
                        }
                    }
                }
            } else if data.starts_with("dl:tm:") {
                // MP3 toggle: flip quality buttons between dl:mp4+mp3:q:uid and dl:mp4:q:uid
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                let parts: Vec<&str> = data.split(':').collect();
                if parts.len() >= 3 {
                    let url_id = parts[2];
                    if let Some(teloxide::types::MaybeInaccessibleMessage::Regular(regular_msg)) = q.message.as_ref() {
                        if let Some(keyboard) = regular_msg.reply_markup() {
                            let mut new_buttons = keyboard.inline_keyboard.clone();

                            let currently_on = new_buttons.iter().flatten().any(|btn| {
                                matches!(&btn.kind,
                                    teloxide::types::InlineKeyboardButtonKind::CallbackData(d)
                                    if d.starts_with("dl:mp4+mp3:") && d.split(':').count() == 4)
                            });

                            for row in &mut new_buttons {
                                for button in row {
                                    if let teloxide::types::InlineKeyboardButtonKind::CallbackData(ref mut cb) =
                                        button.kind
                                    {
                                        if currently_on {
                                            if cb.starts_with("dl:mp4+mp3:") && cb.split(':').count() == 4 {
                                                let without_prefix = cb.trim_start_matches("dl:mp4+mp3:");
                                                *cb = format!("dl:mp4:{}", without_prefix);
                                            }
                                        } else if cb.starts_with("dl:mp4:") && cb.split(':').count() == 4 {
                                            let without_prefix = cb.trim_start_matches("dl:mp4:");
                                            *cb = format!("dl:mp4+mp3:{}", without_prefix);
                                        }
                                    }
                                }
                            }

                            for row in &mut new_buttons {
                                for button in row {
                                    if let teloxide::types::InlineKeyboardButtonKind::CallbackData(ref cb) = button.kind
                                    {
                                        if cb == &format!("dl:tm:{}", url_id) {
                                            button.text = if currently_on {
                                                "☐ 🎵 MP3".to_string()
                                            } else {
                                                "☑ + 🎵 MP3".to_string()
                                            };
                                        }
                                    }
                                }
                            }

                            let new_keyboard = teloxide::types::InlineKeyboardMarkup::new(new_buttons);
                            let _ = crate::telegram::styled::edit_message_reply_markup_styled(
                                &bot,
                                chat_id,
                                message_id,
                                &new_keyboard,
                            )
                            .await;
                            log::info!(
                                "MP3 toggle: {} → {} for user {}",
                                if currently_on { "ON" } else { "OFF" },
                                if currently_on { "OFF" } else { "ON" },
                                chat_id.0
                            );
                        }
                    }
                }
            } else if data.starts_with("dl:") {
                super::callback_download::handle_download_callback(
                    &bot,
                    callback_id.clone(),
                    chat_id,
                    message_id,
                    &data,
                    Arc::clone(&db_pool),
                    Arc::clone(&shared_storage),
                    Arc::clone(&download_queue),
                    Arc::clone(&rate_limiter),
                )
                .await?;
            } else if data.starts_with("pv:") {
                super::callback_preview::handle_preview_callback(
                    &bot,
                    callback_id.clone(),
                    message_clone.as_ref(),
                    chat_id,
                    message_id,
                    &data,
                    Arc::clone(&db_pool),
                    Arc::clone(&shared_storage),
                )
                .await?;
            } else if data.starts_with("history:") {
                handle_history_callback(
                    &bot,
                    callback_id,
                    chat_id,
                    message_id,
                    &data,
                    Arc::clone(&db_pool),
                    Arc::clone(&shared_storage),
                    Arc::clone(&download_queue),
                    Arc::clone(&rate_limiter),
                )
                .await?;
            } else if let Some(format) = data.strip_prefix("export:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                crate::core::export::handle_export(
                    &bot,
                    chat_id,
                    format,
                    Arc::clone(&db_pool),
                    Arc::clone(&shared_storage),
                )
                .await?;
            } else if data.starts_with("vfx:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                if let Err(e) = crate::telegram::voice_effects::handle_voice_effect_callback(
                    &bot,
                    chat_id,
                    message_id,
                    &data,
                    &db_pool,
                    shared_storage.as_ref(),
                )
                .await
                {
                    log::error!("Voice effect callback error: {}", e);
                }
                return Ok(());
            } else if data.starts_with("vp:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                if let Err(e) = crate::telegram::preview::vlipsy::handle_vlipsy_callback(
                    &bot,
                    chat_id,
                    message_id,
                    &data,
                    &db_pool,
                    &shared_storage,
                )
                .await
                {
                    log::error!("Vlipsy preview callback error: {}", e);
                }
                return Ok(());
            } else if data.starts_with("sr:") {
                use crate::telegram::menu::search::handle_search_callback;
                if let Err(e) = handle_search_callback(
                    &bot,
                    callback_id.clone(),
                    chat_id,
                    message_id,
                    &data,
                    Arc::clone(&db_pool),
                    Arc::clone(&shared_storage),
                    Arc::clone(&download_queue),
                )
                .await
                {
                    log::error!("Search callback error: {}", e);
                }
                return Ok(());
            } else if data.starts_with("pw:") {
                use crate::telegram::menu::player::handle_player_callback;
                if let Err(e) = handle_player_callback(
                    &bot,
                    callback_id.clone(),
                    chat_id,
                    message_id,
                    &data,
                    Arc::clone(&db_pool),
                    Arc::clone(&shared_storage),
                    Arc::clone(&download_queue),
                )
                .await
                {
                    log::error!("Player callback error: {}", e);
                }
                return Ok(());
            } else if data.starts_with("pl:") {
                use crate::telegram::menu::playlist::handle_playlist_callback;
                if let Err(e) = handle_playlist_callback(
                    &bot,
                    callback_id.clone(),
                    chat_id,
                    message_id,
                    &data,
                    Arc::clone(&db_pool),
                    Arc::clone(&shared_storage),
                )
                .await
                {
                    log::error!("Playlist callback error: {}", e);
                }
                return Ok(());
            } else if data.starts_with("vault:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                if let Err(e) = crate::telegram::menu::vault::handle_vault_callback(
                    &bot,
                    callback_id.clone(),
                    chat_id,
                    message_id,
                    &data,
                    Arc::clone(&db_pool),
                    Arc::clone(&shared_storage),
                )
                .await
                {
                    log::error!("Vault callback error: {}", e);
                }
                return Ok(());
            } else if data.starts_with("pi:") {
                use crate::telegram::menu::playlist_integrations::handle_playlist_integrations_callback;
                if let Err(e) = handle_playlist_integrations_callback(
                    &bot,
                    callback_id.clone(),
                    chat_id,
                    message_id,
                    &data,
                    Arc::clone(&db_pool),
                    Arc::clone(&shared_storage),
                    Arc::clone(&download_queue),
                )
                .await
                {
                    log::error!("Playlist integrations callback error: {}", e);
                }
                return Ok(());
            } else if data.starts_with("vl:") {
                use crate::telegram::menu::vlipsy::handle_vlipsy_callback;
                if let Err(e) = handle_vlipsy_callback(
                    &bot,
                    callback_id.clone(),
                    chat_id,
                    message_id,
                    &data,
                    Arc::clone(&db_pool),
                    Arc::clone(&shared_storage),
                )
                .await
                {
                    log::error!("Vlipsy callback error: {}", e);
                }
                return Ok(());
            } else if data.starts_with("ringtone:") {
                use crate::telegram::menu::ringtone::handle_ringtone_callback;
                if let Err(e) = handle_ringtone_callback(
                    &bot,
                    callback_id.clone(),
                    chat_id,
                    message_id,
                    &data,
                    Arc::clone(&db_pool),
                    Arc::clone(&shared_storage),
                )
                .await
                {
                    log::error!("Ringtone callback error: {}", e);
                }
                return Ok(());
            } else if data.starts_with("downloads:") {
                use crate::telegram::downloads::handle_downloads_callback;
                handle_downloads_callback(
                    &bot,
                    callback_id.clone(),
                    chat_id,
                    message_id,
                    &data,
                    db_pool.clone(),
                    shared_storage.clone(),
                    q.from.username.clone(),
                    downsub_gateway.clone(),
                    subtitle_cache.clone(),
                )
                .await?;
            } else if data.starts_with("cuts:") {
                use crate::telegram::cuts::handle_cuts_callback;
                handle_cuts_callback(
                    &bot,
                    callback_id.clone(),
                    chat_id,
                    message_id,
                    &data,
                    db_pool.clone(),
                    shared_storage.clone(),
                    q.from.username.clone(),
                )
                .await?;
            } else if data.starts_with("videos:") || data.starts_with("convert:") {
                use crate::telegram::videos::handle_videos_callback;
                handle_videos_callback(
                    &bot,
                    callback_id.clone(),
                    chat_id,
                    message_id,
                    &data,
                    db_pool.clone(),
                    shared_storage.clone(),
                )
                .await?;
            }
        }
    }

    Ok(())
}
