use crate::core::history::handle_history_callback;
use crate::core::rate_limiter::RateLimiter;
use crate::core::subscription::{create_subscription_invoice, show_subscription_info};
use crate::core::types::Plan;
use crate::download::queue::{DownloadQueue, DownloadTask};
use crate::downsub::DownsubGateway;
use crate::extension::ExtensionRegistry;
use crate::i18n;
use crate::storage::cache;
use crate::storage::db::{self, DbPool};
use crate::storage::SubtitleCache;
use crate::telegram::admin;
use crate::telegram::cache as tg_cache;
use crate::telegram::setup_chat_bot_commands;
use crate::telegram::Bot;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, ParseMode};
use teloxide::RequestError;
use url::Url;

use super::audio_effects::{handle_audio_cut_callback, handle_audio_effects_callback};
use super::helpers::{send_queue_position_message, start_download_from_preview};
use super::main_menu::{
    edit_enhanced_main_menu, edit_main_menu, send_main_menu_as_new, show_current_settings_detail,
    show_enhanced_main_menu, show_help_menu,
};
use super::services::{show_extension_detail, show_services_menu};
use super::settings::{
    show_audio_bitrate_menu, show_download_type_menu, show_language_menu, show_progress_bar_style_menu,
    show_video_quality_menu,
};

/// Handles callback queries from the menu inline keyboards.
///
/// Processes button presses, updates user settings, or switches between menus.
pub async fn handle_menu_callback(
    bot: Bot,
    q: CallbackQuery,
    db_pool: Arc<DbPool>,
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
            let lang = i18n::user_lang_from_pool(&db_pool, chat_id.0);
            // Handle audio cut/effects callbacks first
            if data.starts_with("ac:") {
                // Reconstruct CallbackQuery for audio cut handler
                let ac_query = CallbackQuery {
                    id: callback_id.clone(),
                    from: q.from.clone(),
                    message: message_clone,
                    inline_message_id: q.inline_message_id.clone(),
                    chat_instance: q.chat_instance.clone(),
                    data: data_clone,
                    game_short_name: q.game_short_name.clone(),
                };
                if let Err(e) = handle_audio_cut_callback(bot.clone(), ac_query, Arc::clone(&db_pool)).await {
                    log::error!("Audio cut callback error: {}", e);
                }
                return Ok(());
            }
            if data.starts_with("ae:") {
                // Reconstruct CallbackQuery for audio effects handler
                let ae_query = CallbackQuery {
                    id: callback_id.clone(),
                    from: q.from.clone(),
                    message: message_clone,
                    inline_message_id: q.inline_message_id.clone(),
                    chat_instance: q.chat_instance.clone(),
                    data: data_clone,
                    game_short_name: q.game_short_name.clone(),
                };
                if let Err(e) = handle_audio_effects_callback(bot.clone(), ae_query, Arc::clone(&db_pool)).await {
                    log::error!("Audio effects callback error: {}", e);
                }
                return Ok(());
            }

            if data.starts_with("mode:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                // Format: mode:action or mode:action:preview:url_id or mode:action:preview:url_id:preview_msg_id
                let parts: Vec<&str> = data.split(':').collect();
                let action = parts.get(1).unwrap_or(&"");
                let is_from_preview = parts.len() >= 4 && parts[2] == "preview";
                let url_id = if is_from_preview { Some(parts[3]) } else { None };
                let preview_msg_id = if is_from_preview && parts.len() >= 5 {
                    parts[4].parse::<i32>().ok().map(teloxide::types::MessageId)
                } else {
                    None
                };

                match *action {
                    "download_type" => {
                        show_download_type_menu(
                            &bot,
                            chat_id,
                            message_id,
                            Arc::clone(&db_pool),
                            url_id,
                            preview_msg_id,
                        )
                        .await?;
                    }
                    "video_quality" => {
                        show_video_quality_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), url_id).await?;
                    }
                    "audio_bitrate" => {
                        show_audio_bitrate_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), url_id).await?;
                    }
                    "services" => {
                        show_services_menu(&bot, chat_id, message_id, &lang, &extension_registry).await?;
                    }
                    "language" => {
                        show_language_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), url_id).await?;
                    }
                    "subscription" => {
                        // Delete the old message and show subscription info
                        let _ = bot.delete_message(chat_id, message_id).await;
                        let _ = show_subscription_info(&bot, chat_id, Arc::clone(&db_pool)).await;
                    }
                    _ => {}
                }
            } else if data.starts_with("main:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                let action = data.strip_prefix("main:").unwrap_or("");

                match action {
                    "settings" => {
                        // Show the old main menu (current /mode functionality)
                        edit_main_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), None, None).await?;
                    }
                    "current" => {
                        // Show detailed current settings
                        show_current_settings_detail(&bot, chat_id, message_id, Arc::clone(&db_pool)).await?;
                    }
                    "stats" => {
                        // Delete current message and show stats
                        let _ = bot.delete_message(chat_id, message_id).await;
                        let _ = crate::core::stats::show_user_stats(&bot, chat_id, Arc::clone(&db_pool)).await;
                    }
                    "history" => {
                        // Delete current message and show history
                        let _ = bot.delete_message(chat_id, message_id).await;
                        let _ = crate::core::history::show_history(&bot, chat_id, Arc::clone(&db_pool)).await;
                    }
                    "services" => {
                        // Edit message to show services
                        show_services_menu(&bot, chat_id, message_id, &lang, &extension_registry).await?;
                    }
                    "subscription" => {
                        // Delete current message and show subscription info
                        let _ = bot.delete_message(chat_id, message_id).await;
                        let _ = crate::core::subscription::show_subscription_info(&bot, chat_id, Arc::clone(&db_pool))
                            .await;
                    }
                    "help" => {
                        // Edit message to show help
                        show_help_menu(&bot, chat_id, message_id).await?;
                    }
                    "feedback" => {
                        // Delete current message and send feedback prompt
                        let _ = bot.delete_message(chat_id, message_id).await;
                        let _ = crate::telegram::feedback::send_feedback_prompt(&bot, chat_id, &lang).await;
                    }
                    _ => {}
                }
            } else if data.starts_with("ext:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                let parts: Vec<&str> = data.split(':').collect();
                match parts.get(1).copied().unwrap_or("") {
                    "detail" => {
                        if let Some(ext_id) = parts.get(2) {
                            show_extension_detail(&bot, chat_id, message_id, &lang, &extension_registry, ext_id)
                                .await?;
                        }
                    }
                    "back" => {
                        show_services_menu(&bot, chat_id, message_id, &lang, &extension_registry).await?;
                    }
                    _ => {}
                }
            } else if let Some(plan) = data.strip_prefix("subscribe:") {
                log::info!("🔔 Subscribe callback received: data={}, chat_id={}", data, chat_id.0);
                bot.answer_callback_query(callback_id.clone()).await?;
                // Remove "subscribe:" prefix
                log::info!("📌 Extracted plan: {}", plan);
                match plan {
                    "premium" | "vip" => {
                        log::info!("✅ Valid plan '{}', creating invoice for chat_id={}", plan, chat_id.0);
                        // Create an invoice for payment through Telegram Stars
                        match create_subscription_invoice(&bot, chat_id, plan).await {
                            Ok(msg) => {
                                log::info!(
                                    "✅ Invoice created successfully for user {} plan {}. Message ID: {}",
                                    chat_id.0,
                                    plan,
                                    msg.id.0
                                );
                            }
                            Err(e) => {
                                log::error!(
                                    "❌ Failed to create invoice for user {} plan {}: {:?}",
                                    chat_id.0,
                                    plan,
                                    e
                                );
                                log::error!("❌ Error type: {}", e);
                                let _ = bot.send_message(
                                    chat_id,
                                    "❌ An error occurred while creating the invoice. Please try again later or contact the administrator."
                                ).await;
                            }
                        }
                    }
                    _ => {
                        log::warn!("⚠️ Unknown plan requested: {}", plan);
                        bot.answer_callback_query(callback_id).text("Unknown plan").await?;
                    }
                }
            } else if let Some(action) = data.strip_prefix("subscription:") {
                bot.answer_callback_query(callback_id.clone()).await?;
                // Remove "subscription:" prefix
                match action {
                    "cancel" => {
                        // Cancel the user's subscription
                        match crate::core::subscription::cancel_subscription(&bot, chat_id.0, Arc::clone(&db_pool))
                            .await
                        {
                            Ok(_) => {
                                log::info!("Subscription canceled for user {}", chat_id.0);
                                let _ = bot
                                    .send_message(
                                        chat_id,
                                        "✅ Subscription successfully cancelled. It will remain active until the end of the paid period.",
                                    )
                                    .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                                    .await;

                                // Refresh the subscription menu
                                let _ = bot.delete_message(chat_id, message_id).await;
                                let _ = show_subscription_info(&bot, chat_id, Arc::clone(&db_pool)).await;
                            }
                            Err(e) => {
                                log::error!("Failed to cancel subscription: {}", e);

                                // Check if subscription is already non-recurring
                                let message = if e.contains("already non-recurring") {
                                    "ℹ️ You have a one-time subscription without auto-renewal. It will remain active until the end of the paid period."
                                } else {
                                    "❌ Failed to cancel subscription. Please try again later or contact the administrator."
                                };

                                let _ = bot
                                    .send_message(chat_id, message)
                                    .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                                    .await;
                            }
                        }
                    }
                    _ => {
                        bot.answer_callback_query(callback_id).text("Unknown action").await?;
                    }
                }
            } else if let Some(lang_code) = data.strip_prefix("language:select_new:") {
                // Handle language selection for new users (during onboarding)
                if i18n::SUPPORTED_LANGS
                    .iter()
                    .any(|(code, _)| code.eq_ignore_ascii_case(lang_code))
                {
                    if let Ok(conn) = db::get_connection(&db_pool) {
                        let username = q.from.username.clone();
                        // Create user with selected language
                        if let Err(e) = db::create_user_with_language(&conn, chat_id.0, username.clone(), lang_code) {
                            log::warn!("Failed to create user with language: {}", e);
                        } else {
                            log::info!(
                                "New user created with language: chat_id={}, language={}",
                                chat_id.0,
                                lang_code
                            );
                            // Notify admins about new user
                            use crate::telegram::notifications::notify_admin_new_user;
                            let bot_notify = bot.clone();
                            let user_id = chat_id.0;
                            let first_name = q.from.first_name.clone();
                            let lang = lang_code.to_string();
                            tokio::spawn(async move {
                                notify_admin_new_user(
                                    &bot_notify,
                                    user_id,
                                    username.as_deref(),
                                    Some(&first_name),
                                    Some(&lang),
                                    Some("/start → language"),
                                )
                                .await;
                            });
                        }
                    }

                    let new_lang = i18n::lang_from_code(lang_code);
                    if let Err(e) = setup_chat_bot_commands(&bot, chat_id, &new_lang).await {
                        log::warn!("Failed to set chat-specific commands for lang {}: {}", lang_code, e);
                    }
                    let _ = bot
                        .answer_callback_query(callback_id.clone())
                        .text(i18n::t(&new_lang, "menu.language_saved"))
                        .await;

                    // Delete language selection message and show main menu
                    let _ = bot.delete_message(chat_id, message_id).await;
                    let _ = show_enhanced_main_menu(&bot, chat_id, Arc::clone(&db_pool)).await;

                    // Send random voice message in background
                    let bot_voice = bot.clone();
                    let chat_id_voice = chat_id;
                    tokio::spawn(async move {
                        crate::telegram::voice::send_random_voice_message(bot_voice, chat_id_voice).await;
                    });
                } else {
                    let fallback_lang = i18n::lang_from_code("ru");
                    bot.answer_callback_query(callback_id)
                        .text(i18n::t(&fallback_lang, "menu.language_invalid"))
                        .await?;
                }
            } else if let Some(lang_data) = data.strip_prefix("language:set:") {
                let mut parts = lang_data.split(':');
                let lang_code = parts.next().unwrap_or("ru");
                let preview_url_id = parts.next();

                if i18n::SUPPORTED_LANGS
                    .iter()
                    .any(|(code, _)| code.eq_ignore_ascii_case(lang_code))
                {
                    if let Ok(conn) = db::get_connection(&db_pool) {
                        if let Ok(None) = db::get_user(&conn, chat_id.0) {
                            log::info!(
                                "Creating user before setting language: chat_id={}, username={:?}",
                                chat_id.0,
                                q.from.username
                            );
                            let username = q.from.username.clone();
                            if let Err(e) = db::create_user(&conn, chat_id.0, username.clone()) {
                                log::warn!("Failed to create user before setting language: {}", e);
                            } else {
                                // Notify admins about new user
                                use crate::telegram::notifications::notify_admin_new_user;
                                let bot_notify = bot.clone();
                                let user_id = chat_id.0;
                                let first_name = q.from.first_name.clone();
                                let lang = lang_code.to_string();
                                tokio::spawn(async move {
                                    notify_admin_new_user(
                                        &bot_notify,
                                        user_id,
                                        username.as_deref(),
                                        Some(&first_name),
                                        Some(&lang),
                                        Some("language change"),
                                    )
                                    .await;
                                });
                            }
                        }
                        let _ = db::set_user_language(&conn, chat_id.0, lang_code);
                    }

                    let new_lang = i18n::lang_from_code(lang_code);
                    if let Err(e) = setup_chat_bot_commands(&bot, chat_id, &new_lang).await {
                        log::warn!("Failed to set chat-specific commands for lang {}: {}", lang_code, e);
                    }
                    let _ = bot
                        .answer_callback_query(callback_id.clone())
                        .text(i18n::t(&new_lang, "menu.language_saved"))
                        .await;

                    if preview_url_id.is_some() {
                        edit_main_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), preview_url_id, None).await?;
                    } else {
                        edit_enhanced_main_menu(&bot, chat_id, message_id, Arc::clone(&db_pool)).await?;
                    }
                } else {
                    bot.answer_callback_query(callback_id)
                        .text(i18n::t(&lang, "menu.language_invalid"))
                        .await?;
                }
            } else if let Some(quality) = data.strip_prefix("quality:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                // Remove "quality:" prefix
                let conn = db::get_connection(&db_pool)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
                db::set_user_video_quality(&conn, chat_id.0, quality)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                // Update the menu to show new selection
                show_video_quality_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), None).await?;
            } else if data == "send_type:toggle" {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                let conn = db::get_connection(&db_pool)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                // Get the current value and toggle it
                let current_value = db::get_user_send_as_document(&conn, chat_id.0).unwrap_or(0);
                let new_value = if current_value == 0 { 1 } else { 0 };

                db::set_user_send_as_document(&conn, chat_id.0, new_value)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                // Refresh the menu
                show_video_quality_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), None).await?;
            } else if data.starts_with("ct:") {
                // Carousel toggle: ct:{index}:{url_id}:{mask} or ct:all:{url_id}:{mask}
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                let parts: Vec<&str> = data.splitn(4, ':').collect();
                if parts.len() == 4 {
                    let url_id = parts[2];
                    if let Ok(mask) = parts[3].parse::<u32>() {
                        // Figure out carousel_count from the message keyboard
                        // Count item toggle buttons (those starting with ct: and a digit)
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
                // Subscribe to Instagram profile updates — route to subscriptions module
                let username = data.strip_prefix("ig:sub:").unwrap_or("");
                if !username.is_empty() {
                    let registry = std::sync::Arc::new(crate::watcher::WatcherRegistry::default_registry());
                    crate::telegram::subscriptions::show_subscribe_confirm(
                        &bot, chat_id, username, &db_pool, &registry,
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
                    &registry,
                )
                .await;
                return Ok(());
            } else if data == "video:toggle_burn_subs" {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                let conn = db::get_connection(&db_pool)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                // Get the current value and toggle it
                let current_value = db::get_user_burn_subtitles(&conn, chat_id.0).unwrap_or(false);
                let new_value = !current_value;

                db::set_user_burn_subtitles(&conn, chat_id.0, new_value)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                log::info!(
                    "User {} toggled burn_subtitles: {} -> {}",
                    chat_id.0,
                    current_value,
                    new_value
                );

                // Refresh the menu
                show_video_quality_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), None).await?;
            } else if let Some(bitrate) = data.strip_prefix("bitrate:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                // Remove "bitrate:" prefix
                let conn = db::get_connection(&db_pool)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
                db::set_user_audio_bitrate(&conn, chat_id.0, bitrate)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                // Update the menu to show new selection
                show_audio_bitrate_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), None).await?;
            } else if data == "audio_send_type:toggle" {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                let conn = db::get_connection(&db_pool)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                // Get the current value and toggle it
                let current_value = db::get_user_send_audio_as_document(&conn, chat_id.0).unwrap_or(0);
                let new_value = if current_value == 0 { 1 } else { 0 };

                db::set_user_send_audio_as_document(&conn, chat_id.0, new_value)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                // Refresh the menu
                show_audio_bitrate_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), None).await?;
            } else if let Some(style_name) = data.strip_prefix("pbar_style:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                let conn = db::get_connection(&db_pool)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
                db::set_user_progress_bar_style(&conn, chat_id.0, style_name)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                log::info!("User {} set progress bar style to {}", chat_id.0, style_name);

                // Refresh the menu
                show_progress_bar_style_menu(&bot, chat_id, message_id, Arc::clone(&db_pool)).await?;
            } else if data.starts_with("video_send_type:toggle:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;

                // Extract url_id from callback data: video_send_type:toggle:url_id
                let parts: Vec<&str> = data.split(':').collect();
                if parts.len() >= 3 {
                    let url_id = parts[2];

                    let conn = db::get_connection(&db_pool)
                        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                    // Get the current value and toggle it
                    let current_value = db::get_user_send_as_document(&conn, chat_id.0).unwrap_or(0);
                    let new_value = if current_value == 0 { 1 } else { 0 };

                    // Log the change
                    log::info!(
                        "🔄 Video send type toggled for user {}: {} -> {} ({})",
                        chat_id.0,
                        if current_value == 0 { "Media" } else { "Document" },
                        if new_value == 0 { "Media" } else { "Document" },
                        if new_value == 0 { "send_video" } else { "send_document" }
                    );

                    db::set_user_send_as_document(&conn, chat_id.0, new_value)
                        .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                    // Get the current keyboard from the message and update only the toggle button
                    if let Some(teloxide::types::MaybeInaccessibleMessage::Regular(regular_msg)) = q.message.as_ref() {
                        // Get the current keyboard
                        if let Some(keyboard) = regular_msg.reply_markup() {
                            // Clone the keyboard and update the toggle button
                            let mut new_buttons = keyboard.inline_keyboard.clone();

                            // Find and update the toggle button (looking for callback video_send_type:toggle)
                            for row in &mut new_buttons {
                                for button in row {
                                    if let teloxide::types::InlineKeyboardButtonKind::CallbackData(ref cb_data) =
                                        button.kind
                                    {
                                        if cb_data.starts_with("video_send_type:toggle:") {
                                            // Update the button text
                                            button.text = if new_value == 0 {
                                                "📹 Send as: Media ✓".to_string()
                                            } else {
                                                "📄 Send as: Document ✓".to_string()
                                            };
                                            log::debug!("Updated toggle button text to: {}", button.text);
                                        }
                                    }
                                }
                            }

                            // Update only the keyboard without touching text or media
                            let new_keyboard = teloxide::types::InlineKeyboardMarkup::new(new_buttons);
                            let _ = bot
                                .edit_message_reply_markup(chat_id, message_id)
                                .reply_markup(new_keyboard)
                                .await;

                            log::info!(
                                "✅ Updated video preview keyboard for user {} (url_id: {})",
                                chat_id.0,
                                url_id
                            );
                        }
                    }
                }
            } else if data.starts_with("back:") {
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                if data.starts_with("back:preview:") {
                    // Format: back:preview:url_id or back:preview:url_id:preview_msg_id
                    let parts: Vec<&str> = data.split(':').collect();
                    let url_id = parts[2];
                    let _preview_msg_id = if parts.len() >= 4 {
                        parts[3].parse::<i32>().ok().map(teloxide::types::MessageId)
                    } else {
                        None
                    };

                    // Get URL from cache and send new preview with updated format
                    match cache::get_url(&db_pool, url_id).await {
                        Some(url_str) => {
                            match url::Url::parse(&url_str) {
                                Ok(url) => {
                                    let conn = db::get_connection(&db_pool).map_err(|e| {
                                        RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                                    })?;
                                    let current_format = db::get_user_download_format(&conn, chat_id.0)
                                        .unwrap_or_else(|_| "mp3".to_string());
                                    let video_quality = if current_format == "mp4" {
                                        db::get_user_video_quality(&conn, chat_id.0).ok()
                                    } else {
                                        None
                                    };

                                    // Get metadata and update preview
                                    match crate::telegram::preview::get_preview_metadata(
                                        &url,
                                        Some(&current_format),
                                        video_quality.as_deref(),
                                    )
                                    .await
                                    {
                                        Ok(metadata) => {
                                            // Update existing preview message
                                            let time_range = tg_cache::get_time_range(url.as_str()).await;
                                            match crate::telegram::preview::update_preview_message(
                                                &bot,
                                                chat_id,
                                                message_id,
                                                &url,
                                                &metadata,
                                                &current_format,
                                                video_quality.as_deref(),
                                                Arc::clone(&db_pool),
                                                time_range.as_ref(),
                                            )
                                            .await
                                            {
                                                Ok(_) => {}
                                                Err(e) => {
                                                    log::error!("Failed to update preview message: {:?}", e);
                                                    let _ = bot
                                                        .send_message(
                                                            chat_id,
                                                            "Failed to update preview. Please send the link again.",
                                                        )
                                                        .await;
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            log::error!("Failed to get preview metadata: {:?}", e);
                                            let _ = bot
                                                .send_message(
                                                    chat_id,
                                                    "Failed to update preview. Please send the link again.",
                                                )
                                                .await;
                                        }
                                    }
                                }
                                Err(e) => {
                                    log::error!("Failed to parse URL from cache: {}", e);
                                    bot.answer_callback_query(callback_id)
                                        .text("Error: invalid link")
                                        .await?;
                                }
                            }
                        }
                        None => {
                            log::warn!("URL not found in cache for ID: {} (expired or invalid)", url_id);
                            bot.answer_callback_query(callback_id)
                                .text("Link expired, please send it again")
                                .await?;
                        }
                    }
                } else if data.starts_with("back:main:preview:") {
                    // Format: back:main:preview:url_id or back:main:preview:url_id:preview_msg_id
                    let parts: Vec<&str> = data.split(':').collect();
                    let url_id = parts[3];
                    let preview_msg_id = if parts.len() >= 5 {
                        parts[4].parse::<i32>().ok().map(teloxide::types::MessageId)
                    } else {
                        None
                    };
                    edit_main_menu(
                        &bot,
                        chat_id,
                        message_id,
                        Arc::clone(&db_pool),
                        Some(url_id),
                        preview_msg_id,
                    )
                    .await?;
                } else {
                    match data.as_str() {
                        "back:main" => {
                            edit_main_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), None, None).await?;
                        }
                        "back:enhanced_main" => {
                            edit_enhanced_main_menu(&bot, chat_id, message_id, Arc::clone(&db_pool)).await?;
                        }
                        "back:start" => {
                            bot.edit_message_text(
                                chat_id,
                                message_id,
                                "Hey\\! I'm Dora, send me a link and I'll download it ❤️‍🔥",
                            )
                            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                            .await?;
                        }
                        _ => {}
                    }
                }
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

                let conn = db::get_connection(&db_pool)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
                db::set_user_download_format(&conn, chat_id.0, format)
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
                            Arc::clone(&download_queue),
                            Arc::clone(&rate_limiter),
                        )
                        .await?;
                    }
                } else {
                    // Update the menu to show new selection
                    show_download_type_menu(&bot, chat_id, message_id, Arc::clone(&db_pool), None, None).await?;
                }
            } else if data.starts_with("dl:") {
                // Answer callback and delete preview IMMEDIATELY to prevent double-clicks
                // This gives instant visual feedback that the action was processed
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                if let Err(e) = bot.delete_message(chat_id, message_id).await {
                    log::warn!("Failed to delete preview message: {:?}", e);
                }

                // Format: dl:format:url_id (legacy format)
                // Format: dl:format:quality:url_id (video with quality selection)
                // Format: dl:photo:url_id:mask (carousel photo with bitmask)
                let parts: Vec<&str> = data.split(':').collect();

                if parts.len() >= 3 {
                    let format = parts[1];

                    // For carousel photos: dl:photo:url_id:mask (mask is a decimal bitmask)
                    let (url_id, carousel_mask) = if format == "photo" && parts.len() == 4 {
                        // dl:photo:url_id:mask — mask is a u32 bitmask
                        let mask = parts[3].parse::<u32>().ok();
                        (parts[2], mask)
                    } else {
                        (
                            if parts.len() == 3 {
                                parts[2] // dl:format:url_id
                            } else if parts.len() == 4 {
                                parts[3] // dl:format:quality:url_id
                            } else {
                                log::warn!("Invalid dl callback format: {}", data);
                                let _ = bot.send_message(chat_id, "Error: invalid request format").await;
                                return Ok(());
                            },
                            None,
                        )
                    };

                    // Extract quality if provided (new format)
                    let selected_quality = if parts.len() == 4 && format == "mp4" {
                        Some(parts[2].to_string()) // quality from dl:mp4:quality:url_id
                    } else {
                        None
                    };

                    log::debug!(
                        "Download button clicked: chat={}, url_id={}, format={}",
                        chat_id.0,
                        url_id,
                        format
                    );

                    // Get URL from cache by ID
                    match cache::get_url(&db_pool, url_id).await {
                        Some(url_str) => {
                            match Url::parse(&url_str) {
                                Ok(url) => {
                                    let original_message_id = tg_cache::get_link_message_id(&url_str).await;
                                    let time_range = tg_cache::get_time_range(&url_str).await;
                                    // Get user preferences for quality/bitrate and plan
                                    let conn = db::get_connection(&db_pool).map_err(|e| {
                                        RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
                                    })?;
                                    let plan = match db::get_user(&conn, chat_id.0) {
                                        Ok(Some(ref user)) => user.plan,
                                        _ => Plan::default(),
                                    };

                                    // Rate limit disabled - users can download without waiting
                                    let _ = (rate_limiter, &plan); // silence unused warnings

                                    // Handle "mp4+mp3" by adding two tasks to the queue
                                    if format == "mp4+mp3" {
                                        // Task 1: MP4 (video)
                                        let video_quality = if let Some(quality) = selected_quality {
                                            Some(quality)
                                        } else {
                                            Some(
                                                db::get_user_video_quality(&conn, chat_id.0)
                                                    .unwrap_or_else(|_| "best".to_string()),
                                            )
                                        };
                                        let mut task_mp4 = DownloadTask::from_plan(
                                            url.as_str().to_string(),
                                            chat_id,
                                            original_message_id,
                                            true, // is_video = true
                                            "mp4".to_string(),
                                            video_quality,
                                            None, // audio_bitrate is not needed for video
                                            plan.as_str(),
                                        );
                                        task_mp4.time_range = time_range.clone();
                                        download_queue.add_task(task_mp4, Some(Arc::clone(&db_pool))).await;

                                        // Task 2: MP3 (audio)
                                        let audio_bitrate = Some(
                                            db::get_user_audio_bitrate(&conn, chat_id.0)
                                                .unwrap_or_else(|_| "320k".to_string()),
                                        );
                                        let mut task_mp3 = DownloadTask::from_plan(
                                            url.as_str().to_string(),
                                            chat_id,
                                            original_message_id,
                                            false, // is_video = false
                                            "mp3".to_string(),
                                            None, // video_quality is not needed for audio
                                            audio_bitrate,
                                            plan.as_str(),
                                        );
                                        task_mp3.time_range = time_range.clone();
                                        download_queue.add_task(task_mp3, Some(Arc::clone(&db_pool))).await;

                                        log::info!(
                                            "Added 2 tasks to queue for mp4+mp3: MP4 and MP3 for chat {}",
                                            chat_id.0
                                        );

                                        // Send queue position notification and store message ID for later deletion
                                        if let Some(msg_id) = send_queue_position_message(
                                            &bot,
                                            chat_id,
                                            plan.as_str(),
                                            &download_queue,
                                            &db_pool,
                                        )
                                        .await
                                        {
                                            download_queue.set_queue_message_id(chat_id, msg_id.0).await;
                                        }
                                    } else {
                                        // Regular handling for a single format
                                        let video_quality = if format == "mp4" {
                                            if let Some(quality) = selected_quality {
                                                // Quality chosen by the user from preview
                                                Some(quality)
                                            } else {
                                                // Use the user's saved settings
                                                Some(
                                                    db::get_user_video_quality(&conn, chat_id.0)
                                                        .unwrap_or_else(|_| "best".to_string()),
                                                )
                                            }
                                        } else {
                                            None
                                        };
                                        let audio_bitrate = if format == "mp3" {
                                            Some(
                                                db::get_user_audio_bitrate(&conn, chat_id.0)
                                                    .unwrap_or_else(|_| "320k".to_string()),
                                            )
                                        } else {
                                            None
                                        };

                                        // Add task to queue
                                        let is_video = format == "mp4";
                                        let mut task = DownloadTask::from_plan(
                                            url.as_str().to_string(),
                                            chat_id,
                                            original_message_id,
                                            is_video,
                                            format.to_string(),
                                            video_quality,
                                            audio_bitrate,
                                            plan.as_str(),
                                        );
                                        task.time_range = time_range.clone();
                                        task.carousel_mask = carousel_mask;
                                        download_queue.add_task(task, Some(Arc::clone(&db_pool))).await;

                                        // Send queue position notification and store message ID for later deletion
                                        if let Some(msg_id) = send_queue_position_message(
                                            &bot,
                                            chat_id,
                                            plan.as_str(),
                                            &download_queue,
                                            &db_pool,
                                        )
                                        .await
                                        {
                                            download_queue.set_queue_message_id(chat_id, msg_id.0).await;
                                        }
                                    }
                                }
                                Err(e) => {
                                    log::error!("Failed to parse URL from cache: {}", e);
                                    // Preview already deleted, send error as new message
                                    let _ = bot.send_message(chat_id, "❌ Error: invalid link").await;
                                }
                            }
                        }
                        None => {
                            log::warn!("URL not found in cache for ID: {} (expired or invalid)", url_id);
                            // Preview already deleted, send error as new message
                            let _ = bot.send_message(chat_id, "⏰ Link expired, please send it again").await;
                        }
                    }
                }
            } else if data.starts_with("pv:") {
                // Format: pv:action:url_id
                let parts: Vec<&str> = data.splitn(3, ':').collect();
                if parts.len() == 3 {
                    let action = parts[1];
                    match action {
                        "cancel" => {
                            let _ = bot.answer_callback_query(callback_id.clone()).await;
                            // Delete preview message
                            if let Err(e) = bot.delete_message(chat_id, message_id).await {
                                log::warn!("Failed to delete preview message: {:?}", e);
                            }
                        }
                        "set" => {
                            let _ = bot.answer_callback_query(callback_id.clone()).await;
                            let url_id = parts[2]; // Extract url_id from pv:set:url_id
                            let preview_msg_id = message_id; // Save preview message ID for later deletion

                            // Check if message contains photo (media message)
                            // If yes, delete it and send new text message with menu
                            // If no, edit existing text message
                            let has_photo = q
                                .message
                                .as_ref()
                                .and_then(|m| match m {
                                    teloxide::types::MaybeInaccessibleMessage::Regular(msg) => msg.photo(),
                                    _ => None,
                                })
                                .is_some();

                            if has_photo {
                                // Delete media message and send new text message
                                if let Err(e) = bot.delete_message(chat_id, message_id).await {
                                    log::warn!("Failed to delete preview message: {:?}", e);
                                }
                                // Send new text message with main settings menu, pass url_id and preview_msg_id
                                send_main_menu_as_new(
                                    &bot,
                                    chat_id,
                                    Arc::clone(&db_pool),
                                    Some(url_id),
                                    Some(preview_msg_id),
                                )
                                .await?;
                            } else {
                                // Edit existing text message to show main menu, pass url_id and preview_msg_id
                                edit_main_menu(
                                    &bot,
                                    chat_id,
                                    message_id,
                                    Arc::clone(&db_pool),
                                    Some(url_id),
                                    Some(preview_msg_id),
                                )
                                .await?;
                            }
                        }
                        _ => {
                            bot.answer_callback_query(callback_id.clone())
                                .text("Unknown action")
                                .await?;
                        }
                    }
                }
            } else if data.starts_with("history:") {
                // Handle history callbacks
                handle_history_callback(
                    &bot,
                    callback_id,
                    chat_id,
                    message_id,
                    &data,
                    Arc::clone(&db_pool),
                    Arc::clone(&download_queue),
                    Arc::clone(&rate_limiter),
                )
                .await?;
            } else if let Some(format) = data.strip_prefix("export:") {
                // Handle export callbacks
                let _ = bot.answer_callback_query(callback_id.clone()).await;
                // Remove "export:" prefix
                crate::core::export::handle_export(&bot, chat_id, format, Arc::clone(&db_pool)).await?;
            } else if data.starts_with("analytics:") {
                // Handle analytics callback buttons
                let _ = bot.answer_callback_query(callback_id.clone()).await;

                // Check administrator privileges
                let is_admin = i64::try_from(q.from.id.0).ok().map(admin::is_admin).unwrap_or(false);

                if !is_admin {
                    bot.send_message(chat_id, "❌ You don't have permission to execute this command.")
                        .await?;
                    return Ok(());
                }

                match data.as_str() {
                    "analytics:refresh" => {
                        // Re-generate and update analytics dashboard
                        use crate::telegram::analytics::generate_analytics_dashboard;
                        let dashboard = generate_analytics_dashboard(&db_pool).await;

                        let keyboard = InlineKeyboardMarkup::new(vec![
                            vec![
                                crate::telegram::cb("🔄 Refresh", "analytics:refresh"),
                                crate::telegram::cb("📊 Details", "analytics:details"),
                            ],
                            vec![crate::telegram::cb("🔙 Close", "analytics:close")],
                        ]);

                        bot.edit_message_text(chat_id, message_id, dashboard)
                            .parse_mode(ParseMode::MarkdownV2)
                            .reply_markup(keyboard)
                            .await?;
                    }
                    "analytics:details" => {
                        // Show detailed metrics menu
                        let details_text = "📊 *Detailed Metrics*\n\nSelect a category:";
                        let keyboard = InlineKeyboardMarkup::new(vec![
                            vec![crate::telegram::cb("⚡ Performance", "metrics:performance")],
                            vec![crate::telegram::cb("💰 Business", "metrics:business")],
                            vec![crate::telegram::cb("👥 Engagement", "metrics:engagement")],
                            vec![crate::telegram::cb("🔙 Back", "analytics:refresh")],
                        ]);

                        bot.edit_message_text(chat_id, message_id, details_text)
                            .parse_mode(ParseMode::MarkdownV2)
                            .reply_markup(keyboard)
                            .await?;
                    }
                    "analytics:close" => {
                        // Delete the message
                        let _ = bot.delete_message(chat_id, message_id).await;
                    }
                    _ => {}
                }
            } else if data.starts_with("metrics:") {
                // Handle detailed metrics category callbacks
                let _ = bot.answer_callback_query(callback_id.clone()).await;

                // Check administrator privileges
                let is_admin = i64::try_from(q.from.id.0).ok().map(admin::is_admin).unwrap_or(false);

                if !is_admin {
                    bot.send_message(chat_id, "❌ You don't have permission to execute this command.")
                        .await?;
                    return Ok(());
                }

                let category = data.strip_prefix("metrics:").unwrap_or("");

                use crate::telegram::analytics::generate_metrics_report;
                let metrics_text = generate_metrics_report(&db_pool, Some(category.to_string())).await;

                let keyboard = InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb(
                    "🔙 To main dashboard",
                    "analytics:refresh",
                )]]);

                bot.edit_message_text(chat_id, message_id, metrics_text)
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(keyboard)
                    .await?;
            } else if data.starts_with("downloads:") {
                // Handle downloads callback queries
                use crate::telegram::downloads::handle_downloads_callback;
                handle_downloads_callback(
                    &bot,
                    callback_id.clone(),
                    chat_id,
                    message_id,
                    &data,
                    db_pool.clone(),
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
                    q.from.username.clone(),
                )
                .await?;
            } else if data.starts_with("videos:") || data.starts_with("convert:") {
                // Handle videos and conversion callback queries
                use crate::telegram::videos::handle_videos_callback;
                handle_videos_callback(&bot, callback_id.clone(), chat_id, message_id, &data, db_pool.clone()).await?;
            } else if data.starts_with("admin:") {
                // Handle admin panel callbacks
                let _ = bot.answer_callback_query(callback_id.clone()).await;

                // Check administrator privileges
                let is_admin = i64::try_from(q.from.id.0).ok().map(admin::is_admin).unwrap_or(false);

                if !is_admin {
                    bot.send_message(chat_id, "❌ You don't have permission to execute this command.")
                        .await?;
                    return Ok(());
                }

                // Handle yt-dlp version/update callbacks
                // Handle browser/cookie manager callbacks
                if data.starts_with("admin:browser_") {
                    if let Err(e) =
                        admin::handle_browser_callback(&bot, callback_id.to_string(), chat_id, message_id, &data).await
                    {
                        log::error!("Failed to handle browser callback: {}", e);
                    }
                    return Ok(());
                }

                if data == "admin:update_ytdlp" {
                    if let Err(e) = admin::handle_update_ytdlp_callback(&bot, chat_id, message_id).await {
                        log::error!("Failed to handle update_ytdlp callback: {}", e);
                    }
                    return Ok(());
                }

                if data == "admin:check_ytdlp_version" {
                    if let Err(e) = admin::handle_check_ytdlp_version_callback(&bot, chat_id, message_id).await {
                        log::error!("Failed to handle check_ytdlp_version callback: {}", e);
                    }
                    return Ok(());
                }

                if data == "admin:test_cookies" {
                    if let Err(e) = admin::handle_test_cookies_callback(&bot, chat_id, message_id).await {
                        log::error!("Failed to handle test_cookies callback: {}", e);
                    }
                    return Ok(());
                }

                if let Some(user_id_str) = data.strip_prefix("admin:user:") {
                    // Show the management menu for a specific user

                    if let Ok(user_id) = user_id_str.parse::<i64>() {
                        match db::get_connection(&db_pool) {
                            Ok(conn) => {
                                match db::get_user(&conn, user_id) {
                                    Ok(Some(user)) => {
                                        let username_display = user
                                            .username
                                            .as_ref()
                                            .map(|u| format!("@{}", u))
                                            .unwrap_or_else(|| format!("ID: {}", user.telegram_id));

                                        let plan_emoji = user.plan.emoji();

                                        let sub_status = if user.telegram_charge_id.is_some() {
                                            if user.is_recurring {
                                                "💫🔄 Active subscription \\(auto-renewal\\)"
                                            } else {
                                                "💫 Active subscription \\(one-time\\)"
                                            }
                                        } else {
                                            "🔒 No subscription"
                                        };

                                        let expires_info = if let Some(expires) = &user.subscription_expires_at {
                                            let escaped_expires = expires.replace("-", "\\-").replace(":", "\\:");
                                            if user.is_recurring {
                                                format!("\n📅 Next charge: {}", escaped_expires)
                                            } else {
                                                format!("\n📅 Expires: {}", escaped_expires)
                                            }
                                        } else {
                                            String::new()
                                        };

                                        // Build an action keyboard
                                        let keyboard = InlineKeyboardMarkup::new(vec![
                                            vec![crate::telegram::cb(
                                                "🌟 Set Free",
                                                format!("admin:setplan:{}:free", user_id),
                                            )],
                                            vec![crate::telegram::cb(
                                                "⭐ Set Premium",
                                                format!("admin:setplan:{}:premium", user_id),
                                            )],
                                            vec![crate::telegram::cb(
                                                "👑 Set VIP",
                                                format!("admin:setplan:{}:vip", user_id),
                                            )],
                                            vec![crate::telegram::cb("🔙 Back to list", "admin:back")],
                                        ]);

                                        let _ = bot
                                            .edit_message_text(
                                                chat_id,
                                                message_id,
                                                format!(
                                                    "👤 *User Management*\n\n\
                                    User: {}\n\
                                    ID: `{}`\n\
                                    Current plan: {} {}\n\
                                    Status: {}{}\n\n\
                                    Choose action:",
                                                    username_display,
                                                    user.telegram_id,
                                                    plan_emoji,
                                                    user.plan,
                                                    sub_status,
                                                    expires_info
                                                ),
                                            )
                                            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                                            .reply_markup(keyboard)
                                            .await;
                                    }
                                    Ok(None) => {}
                                    Err(e) => {
                                        log::error!("Failed to get user {}: {}", user_id, e);
                                    }
                                }
                            }
                            Err(e) => {
                                log::error!("Failed to get database connection: {}", e);
                            }
                        }
                    }
                } else if data.starts_with("admin:setplan:") {
                    // Change the user's plan
                    let parts: Vec<&str> = data.split(':').collect();
                    if parts.len() == 4 {
                        if let Ok(user_id) = parts[2].parse::<i64>() {
                            let new_plan = parts[3];

                            match db::get_connection(&db_pool) {
                                Ok(conn) => {
                                    match db::update_user_plan(&conn, user_id, new_plan) {
                                        Ok(_) => {
                                            let plan_emoji = match new_plan {
                                                "premium" => "⭐",
                                                "vip" => "👑",
                                                _ => "🌟",
                                            };
                                            let plan_name = match new_plan {
                                                "premium" => "Premium",
                                                "vip" => "VIP",
                                                _ => "Free",
                                            };

                                            // Send a notification to the user
                                            let user_chat_id = teloxide::types::ChatId(user_id);
                                            let _ = bot
                                                .send_message(
                                                    user_chat_id,
                                                    format!(
                                                        "💳 *Subscription Plan Change*\n\n\
                                                    Your plan has been changed by the administrator.\n\n\
                                                    *New plan:* {} {}\n\n\
                                                    Changes take effect immediately! 🎉",
                                                        plan_emoji, plan_name
                                                    ),
                                                )
                                                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                                                .await;

                                            let _ = bot
                                                .edit_message_text(
                                                    chat_id,
                                                    message_id,
                                                    format!(
                                                        "✅ User {} plan changed to {} {}",
                                                        user_id, plan_emoji, new_plan
                                                    ),
                                                )
                                                .await;
                                        }
                                        Err(e) => {
                                            log::error!("Failed to update user plan: {}", e);
                                        }
                                    }
                                }
                                Err(e) => {
                                    log::error!("Failed to get database connection: {}", e);
                                }
                            }
                        }
                    }
                } else if data == "admin:back" {
                    // Return to the user list
                    match db::get_connection(&db_pool) {
                        Ok(conn) => match db::get_all_users(&conn) {
                            Ok(users) => {
                                let mut keyboard_rows = Vec::new();
                                let mut current_row = Vec::new();

                                for user in users.iter().take(20) {
                                    let username_display = user
                                        .username
                                        .as_ref()
                                        .map(|u| format!("@{}", u))
                                        .unwrap_or_else(|| format!("ID:{}", user.telegram_id));

                                    let plan_emoji = user.plan.emoji();

                                    let button_text = format!("{} {}", plan_emoji, username_display);
                                    let callback_data = format!("admin:user:{}", user.telegram_id);

                                    current_row.push(crate::telegram::cb(button_text, callback_data));

                                    if current_row.len() == 2 {
                                        keyboard_rows.push(current_row.clone());
                                        current_row.clear();
                                    }
                                }

                                if !current_row.is_empty() {
                                    keyboard_rows.push(current_row);
                                }

                                let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

                                let _ = bot
                                    .edit_message_text(
                                        chat_id,
                                        message_id,
                                        format!(
                                            "🔧 *User Management Panel*\n\n\
                            Select a user to manage:\n\n\
                            Shown: {} of {}\n\n\
                            💡 To manage a specific user use:\n\
                            `/setplan <user_id> <plan>`",
                                            users.len().min(20),
                                            users.len()
                                        ),
                                    )
                                    .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                                    .reply_markup(keyboard)
                                    .await;
                            }
                            Err(e) => {
                                log::error!("Failed to get users: {}", e);
                            }
                        },
                        Err(e) => {
                            log::error!("Failed to get database connection: {}", e);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
