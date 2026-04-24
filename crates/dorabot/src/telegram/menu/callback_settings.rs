use crate::core::subscription::{create_subscription_invoice, show_subscription_info};
use crate::extension::ExtensionRegistry;
use crate::i18n;
use crate::storage::cache;
use crate::storage::db::DbPool;
use crate::storage::SharedStorage;
use crate::telegram::setup_chat_bot_commands;
use crate::telegram::{Bot, BotExt};
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::CallbackQueryId;
use teloxide::RequestError;
use unic_langid::LanguageIdentifier;

use super::main_menu::{
    edit_enhanced_main_menu, edit_main_menu, show_current_settings_detail, show_enhanced_main_menu, show_help_menu,
};
use super::services::{show_extension_detail, show_services_menu};
use super::settings::{
    show_audio_bitrate_menu, show_download_type_menu, show_language_menu, show_progress_bar_style_menu,
    show_subtitle_style_menu, show_video_quality_menu,
};

/// Handles settings-related callback queries: `mode:`, `main:`, `ext:`, `subscribe:`,
/// `subscription:`, `language:select_new:`, `language:set:`, `quality:`, `send_type:toggle`,
/// `video:toggle_burn_subs`, `settings:toggle_experimental`, `bitrate:`, `audio_send_type:toggle`,
/// `subtitle:`, `pbar_style:`, `video_send_type:toggle:`, and `back:` prefixes.
///
/// Returns `Ok(true)` if the callback was handled, `Ok(false)` if it was not recognized.
#[allow(clippy::too_many_arguments)]
pub async fn handle_settings_callback(
    bot: &Bot,
    callback_id: &CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    data: &str,
    from: &teloxide::types::User,
    q_message: Option<&teloxide::types::MaybeInaccessibleMessage>,
    lang: &LanguageIdentifier,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    extension_registry: Arc<ExtensionRegistry>,
) -> ResponseResult<bool> {
    if data.starts_with("mode:") {
        handle_settings_mode(
            bot,
            callback_id,
            chat_id,
            message_id,
            data,
            lang,
            db_pool,
            shared_storage,
            extension_registry,
        )
        .await?;
        return Ok(true);
    }

    if data.starts_with("main:") {
        handle_settings_main(
            bot,
            callback_id,
            chat_id,
            message_id,
            data,
            lang,
            db_pool,
            shared_storage,
            extension_registry,
        )
        .await?;
        return Ok(true);
    }

    if data.starts_with("ext:") {
        handle_settings_ext(bot, callback_id, chat_id, message_id, data, lang, extension_registry).await?;
        return Ok(true);
    }

    if let Some(plan) = data.strip_prefix("subscribe:") {
        handle_settings_subscribe(bot, callback_id, chat_id, data, plan).await?;
        return Ok(true);
    }

    if let Some(action) = data.strip_prefix("subscription:") {
        handle_settings_subscription(bot, callback_id, chat_id, message_id, action, db_pool, shared_storage).await?;
        return Ok(true);
    }

    if let Some(lang_code) = data.strip_prefix("language:select_new:") {
        handle_settings_language_select_new(
            bot,
            callback_id,
            chat_id,
            message_id,
            lang_code,
            from,
            db_pool,
            shared_storage,
        )
        .await?;
        return Ok(true);
    }

    if let Some(lang_data) = data.strip_prefix("language:set:") {
        handle_settings_language_set(
            bot,
            callback_id,
            chat_id,
            message_id,
            lang_data,
            from,
            lang,
            db_pool,
            shared_storage,
        )
        .await?;
        return Ok(true);
    }

    if let Some(quality) = data.strip_prefix("quality:") {
        handle_settings_quality(bot, callback_id, chat_id, message_id, quality, db_pool, shared_storage).await?;
        return Ok(true);
    }

    if data == "send_type:toggle" {
        handle_settings_send_type_toggle(bot, callback_id, chat_id, message_id, db_pool, shared_storage).await?;
        return Ok(true);
    }

    if data == "video:toggle_burn_subs" {
        handle_settings_video_toggle_burn_subs(bot, callback_id, chat_id, message_id, db_pool, shared_storage).await?;
        return Ok(true);
    }

    if data == "video:toggle_no_caption" {
        handle_settings_video_toggle_no_caption(bot, callback_id, chat_id, message_id, db_pool, shared_storage).await?;
        return Ok(true);
    }

    if let Some(bitrate) = data.strip_prefix("bitrate:") {
        handle_settings_bitrate(bot, callback_id, chat_id, message_id, bitrate, db_pool, shared_storage).await?;
        return Ok(true);
    }

    if data == "audio_send_type:toggle" {
        handle_settings_audio_send_type_toggle(bot, callback_id, chat_id, message_id, db_pool, shared_storage).await?;
        return Ok(true);
    }

    if let Some(setting) = data.strip_prefix("subtitle:") {
        handle_settings_subtitle(bot, callback_id, chat_id, message_id, setting, db_pool, shared_storage).await?;
        return Ok(true);
    }

    if let Some(style_name) = data.strip_prefix("pbar_style:") {
        handle_settings_pbar_style(
            bot,
            callback_id,
            chat_id,
            message_id,
            style_name,
            db_pool,
            shared_storage,
        )
        .await?;
        return Ok(true);
    }

    if data.starts_with("video_send_type:toggle:") {
        handle_settings_video_send_type_toggle(bot, callback_id, chat_id, message_id, data, q_message, shared_storage)
            .await?;
        return Ok(true);
    }

    if data.starts_with("back:") {
        handle_settings_back(bot, callback_id, chat_id, message_id, data, db_pool, shared_storage).await?;
        return Ok(true);
    }

    Ok(false)
}

/// Handles `mode:` callbacks — opens a setting-category menu (download type,
/// video quality, audio bitrate, services, language, subtitle style, progress
/// bar style, or subscription info).
#[allow(clippy::too_many_arguments)]
async fn handle_settings_mode(
    bot: &Bot,
    callback_id: &CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    data: &str,
    lang: &LanguageIdentifier,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    extension_registry: Arc<ExtensionRegistry>,
) -> ResponseResult<()> {
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
                bot,
                chat_id,
                message_id,
                Arc::clone(&db_pool),
                Arc::clone(&shared_storage),
                url_id,
                preview_msg_id,
            )
            .await?;
        }
        "video_quality" => {
            show_video_quality_menu(
                bot,
                chat_id,
                message_id,
                Arc::clone(&db_pool),
                Arc::clone(&shared_storage),
                url_id,
            )
            .await?;
        }
        "audio_bitrate" => {
            show_audio_bitrate_menu(
                bot,
                chat_id,
                message_id,
                Arc::clone(&db_pool),
                Arc::clone(&shared_storage),
                url_id,
            )
            .await?;
        }
        "services" => {
            show_services_menu(bot, chat_id, message_id, lang, &extension_registry).await?;
        }
        "language" => {
            show_language_menu(
                bot,
                chat_id,
                message_id,
                Arc::clone(&db_pool),
                Arc::clone(&shared_storage),
                url_id,
            )
            .await?;
        }
        "subtitle_style" => {
            show_subtitle_style_menu(
                bot,
                chat_id,
                message_id,
                Arc::clone(&db_pool),
                Arc::clone(&shared_storage),
            )
            .await?;
        }
        "progress_bar_style" => {
            show_progress_bar_style_menu(
                bot,
                chat_id,
                message_id,
                Arc::clone(&db_pool),
                Arc::clone(&shared_storage),
            )
            .await?;
        }
        "subscription" => {
            // Delete the old message and show subscription info
            bot.try_delete(chat_id, message_id).await;
            let _ = show_subscription_info(bot, chat_id, Arc::clone(&db_pool), Arc::clone(&shared_storage)).await;
        }
        _ => {}
    }
    Ok(())
}

/// Handles `main:` callbacks — main-menu sub-actions (settings, current,
/// stats, history, services, subscription, help, feedback).
#[allow(clippy::too_many_arguments)]
async fn handle_settings_main(
    bot: &Bot,
    callback_id: &CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    data: &str,
    lang: &LanguageIdentifier,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    extension_registry: Arc<ExtensionRegistry>,
) -> ResponseResult<()> {
    let _ = bot.answer_callback_query(callback_id.clone()).await;
    let action = data.strip_prefix("main:").unwrap_or("");

    match action {
        "settings" => {
            edit_main_menu(
                bot,
                chat_id,
                message_id,
                Arc::clone(&db_pool),
                Arc::clone(&shared_storage),
                None,
                None,
            )
            .await?;
        }
        "current" => {
            show_current_settings_detail(
                bot,
                chat_id,
                message_id,
                Arc::clone(&db_pool),
                Arc::clone(&shared_storage),
            )
            .await?;
        }
        "stats" => {
            bot.try_delete(chat_id, message_id).await;
            let _ =
                crate::core::stats::show_user_stats(bot, chat_id, Arc::clone(&db_pool), Arc::clone(&shared_storage))
                    .await;
        }
        "history" => {
            bot.try_delete(chat_id, message_id).await;
            let _ = crate::core::history::show_history(bot, chat_id, Arc::clone(&db_pool), Arc::clone(&shared_storage))
                .await;
        }
        "services" => {
            show_services_menu(bot, chat_id, message_id, lang, &extension_registry).await?;
        }
        "subscription" => {
            bot.try_delete(chat_id, message_id).await;
            let _ = crate::core::subscription::show_subscription_info(
                bot,
                chat_id,
                Arc::clone(&db_pool),
                Arc::clone(&shared_storage),
            )
            .await;
        }
        "help" => {
            show_help_menu(bot, chat_id, message_id).await?;
        }
        "feedback" => {
            bot.try_delete(chat_id, message_id).await;
            let _ = crate::telegram::feedback::send_feedback_prompt(bot, chat_id, lang, &shared_storage).await;
        }
        _ => {}
    }
    Ok(())
}

/// Handles `ext:` callbacks — extension detail view and back navigation.
#[allow(clippy::too_many_arguments)]
async fn handle_settings_ext(
    bot: &Bot,
    callback_id: &CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    data: &str,
    lang: &LanguageIdentifier,
    extension_registry: Arc<ExtensionRegistry>,
) -> ResponseResult<()> {
    let _ = bot.answer_callback_query(callback_id.clone()).await;
    let parts: Vec<&str> = data.split(':').collect();
    match parts.get(1).copied().unwrap_or("") {
        "detail" => {
            if let Some(ext_id) = parts.get(2) {
                show_extension_detail(bot, chat_id, message_id, lang, &extension_registry, ext_id).await?;
            }
        }
        "back" => {
            show_services_menu(bot, chat_id, message_id, lang, &extension_registry).await?;
        }
        _ => {}
    }
    Ok(())
}

/// Handles `subscribe:` callbacks — creates a Telegram Stars invoice for
/// premium/vip plans.
async fn handle_settings_subscribe(
    bot: &Bot,
    callback_id: &CallbackQueryId,
    chat_id: ChatId,
    data: &str,
    plan: &str,
) -> ResponseResult<()> {
    log::info!("🔔 Subscribe callback received: data={}, chat_id={}", data, chat_id.0);
    bot.answer_callback_query(callback_id.clone()).await?;
    log::info!("📌 Extracted plan: {}", plan);
    match plan {
        "premium" | "vip" => {
            log::info!("✅ Valid plan '{}', creating invoice for chat_id={}", plan, chat_id.0);
            match create_subscription_invoice(bot, chat_id, plan).await {
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
            bot.answer_callback_query(callback_id.clone())
                .text("Unknown plan")
                .await?;
        }
    }
    Ok(())
}

/// Handles `subscription:` callbacks — subscription info actions such as
/// cancelling an active subscription.
#[allow(clippy::too_many_arguments)]
async fn handle_settings_subscription(
    bot: &Bot,
    callback_id: &CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    action: &str,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    bot.answer_callback_query(callback_id.clone()).await?;
    match action {
        "cancel" => {
            match crate::core::subscription::cancel_subscription(bot, chat_id.0, Arc::clone(&shared_storage)).await {
                Ok(_) => {
                    log::info!("Subscription canceled for user {}", chat_id.0);
                    let _ = bot
                        .send_message(
                            chat_id,
                            "✅ Subscription successfully cancelled. It will remain active until the end of the paid period.",
                        )
                        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                        .await;

                    bot.try_delete(chat_id, message_id).await;
                    let _ =
                        show_subscription_info(bot, chat_id, Arc::clone(&db_pool), Arc::clone(&shared_storage)).await;
                }
                Err(e) => {
                    log::error!("Failed to cancel subscription: {}", e);

                    let message = if e.to_string().contains("already non-recurring") {
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
            bot.answer_callback_query(callback_id.clone())
                .text("Unknown action")
                .await?;
        }
    }
    Ok(())
}

/// Handles `language:select_new:` callbacks — first-time language selection
/// during onboarding. Creates the user, notifies admins, and shows the
/// enhanced main menu.
#[allow(clippy::too_many_arguments)]
async fn handle_settings_language_select_new(
    bot: &Bot,
    callback_id: &CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    lang_code: &str,
    from: &teloxide::types::User,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    if i18n::SUPPORTED_LANGS
        .iter()
        .any(|(code, _)| code.eq_ignore_ascii_case(lang_code))
    {
        let username = from.username.clone();
        if let Err(e) = shared_storage
            .create_user_with_language(chat_id.0, username.clone(), Some(lang_code))
            .await
        {
            log::warn!("Failed to create user with language: {}", e);
            let _ = bot
                .answer_callback_query(callback_id.clone())
                .text("Failed to save language. Please try again.")
                .await;
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
            let first_name = from.first_name.clone();
            let lang_str = lang_code.to_string();
            tokio::spawn(async move {
                notify_admin_new_user(
                    &bot_notify,
                    user_id,
                    username.as_deref(),
                    Some(&first_name),
                    Some(&lang_str),
                    Some("/start → language"),
                )
                .await;
            });

            let new_lang = i18n::lang_from_code(lang_code);
            if let Err(e) = setup_chat_bot_commands(bot, chat_id, &new_lang).await {
                log::warn!("Failed to set chat-specific commands for lang {}: {}", lang_code, e);
            }
            let _ = bot
                .answer_callback_query(callback_id.clone())
                .text(i18n::t(&new_lang, "menu.language_saved"))
                .await;

            bot.try_delete(chat_id, message_id).await;
            let _ = show_enhanced_main_menu(bot, chat_id, Arc::clone(&db_pool), Arc::clone(&shared_storage)).await;

            // Send random voice message in background
            let bot_voice = bot.clone();
            let chat_id_voice = chat_id;
            tokio::spawn(async move {
                crate::telegram::voice::send_random_voice_message(bot_voice, chat_id_voice).await;
            });
        }
    } else {
        let fallback_lang = i18n::lang_from_code("ru");
        bot.answer_callback_query(callback_id.clone())
            .text(i18n::t(&fallback_lang, "menu.language_invalid"))
            .await?;
    }
    Ok(())
}

/// Handles `language:set:` callbacks — language change for an existing user
/// (with optional preview URL id for post-preview flows).
#[allow(clippy::too_many_arguments)]
async fn handle_settings_language_set(
    bot: &Bot,
    callback_id: &CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    lang_data: &str,
    from: &teloxide::types::User,
    lang: &LanguageIdentifier,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    let mut parts = lang_data.split(':');
    let lang_code = parts.next().unwrap_or("ru");
    let preview_url_id = parts.next();

    if i18n::SUPPORTED_LANGS
        .iter()
        .any(|(code, _)| code.eq_ignore_ascii_case(lang_code))
    {
        if let Ok(None) = shared_storage.get_user(chat_id.0).await {
            log::info!(
                "Creating user before setting language: chat_id={}, username={:?}",
                chat_id.0,
                from.username
            );
            let username = from.username.clone();
            if let Err(e) = shared_storage
                .create_user_with_language(chat_id.0, username.clone(), Some(lang_code))
                .await
            {
                log::warn!("Failed to create user before setting language: {}", e);
            } else {
                use crate::telegram::notifications::notify_admin_new_user;
                let bot_notify = bot.clone();
                let user_id = chat_id.0;
                let first_name = from.first_name.clone();
                let lang_str = lang_code.to_string();
                tokio::spawn(async move {
                    notify_admin_new_user(
                        &bot_notify,
                        user_id,
                        username.as_deref(),
                        Some(&first_name),
                        Some(&lang_str),
                        Some("language change"),
                    )
                    .await;
                });
            }
        } else {
            let _ = shared_storage.set_user_language(chat_id.0, lang_code).await;
        }

        let new_lang = i18n::lang_from_code(lang_code);
        if let Err(e) = setup_chat_bot_commands(bot, chat_id, &new_lang).await {
            log::warn!("Failed to set chat-specific commands for lang {}: {}", lang_code, e);
        }
        let _ = bot
            .answer_callback_query(callback_id.clone())
            .text(i18n::t(&new_lang, "menu.language_saved"))
            .await;

        if preview_url_id.is_some() {
            edit_main_menu(
                bot,
                chat_id,
                message_id,
                Arc::clone(&db_pool),
                Arc::clone(&shared_storage),
                preview_url_id,
                None,
            )
            .await?;
        } else {
            edit_enhanced_main_menu(
                bot,
                chat_id,
                message_id,
                Arc::clone(&db_pool),
                Arc::clone(&shared_storage),
            )
            .await?;
        }
    } else {
        bot.answer_callback_query(callback_id.clone())
            .text(i18n::t(lang, "menu.language_invalid"))
            .await?;
    }
    Ok(())
}

/// Handles `quality:` callbacks — persists the chosen video quality and
/// re-renders the quality menu.
#[allow(clippy::too_many_arguments)]
async fn handle_settings_quality(
    bot: &Bot,
    callback_id: &CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    quality: &str,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    let _ = bot.answer_callback_query(callback_id.clone()).await;
    const VALID_QUALITIES: &[&str] = &["best", "1080p", "720p", "480p", "360p"];
    if !VALID_QUALITIES.contains(&quality) {
        log::warn!("Rejected invalid quality value from user {}: {:?}", chat_id.0, quality);
        return Ok(());
    }
    shared_storage
        .set_user_video_quality(chat_id.0, quality)
        .await
        .map_err(db_err)?;

    show_video_quality_menu(
        bot,
        chat_id,
        message_id,
        Arc::clone(&db_pool),
        Arc::clone(&shared_storage),
        None,
    )
    .await?;
    Ok(())
}

/// Handles `send_type:toggle` — toggles "send as video/document" and
/// re-renders the quality menu.
async fn handle_settings_send_type_toggle(
    bot: &Bot,
    callback_id: &CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    let _ = bot.answer_callback_query(callback_id.clone()).await;
    let current_value = shared_storage.get_user_send_as_document(chat_id.0).await.unwrap_or(0);
    let new_value = if current_value == 0 { 1 } else { 0 };

    shared_storage
        .set_user_send_as_document(chat_id.0, new_value)
        .await
        .map_err(db_err)?;

    show_video_quality_menu(
        bot,
        chat_id,
        message_id,
        Arc::clone(&db_pool),
        Arc::clone(&shared_storage),
        None,
    )
    .await?;
    Ok(())
}

/// Handles `video:toggle_burn_subs` — toggles the burn-subtitles-into-video
/// flag and re-renders the quality menu.
async fn handle_settings_video_toggle_burn_subs(
    bot: &Bot,
    callback_id: &CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    let _ = bot.answer_callback_query(callback_id.clone()).await;
    let current_value = shared_storage.get_user_burn_subtitles(chat_id.0).await.unwrap_or(false);
    let new_value = !current_value;

    shared_storage
        .set_user_burn_subtitles(chat_id.0, new_value)
        .await
        .map_err(db_err)?;

    log::info!(
        "User {} toggled burn_subtitles: {} -> {}",
        chat_id.0,
        current_value,
        new_value
    );

    show_video_quality_menu(
        bot,
        chat_id,
        message_id,
        Arc::clone(&db_pool),
        Arc::clone(&shared_storage),
        None,
    )
    .await?;
    Ok(())
}

/// Handles `video:toggle_no_caption` — toggles per-user preference to
/// suppress the Markdown caption when sending downloaded videos, then
/// re-renders the quality menu so the updated status is visible.
async fn handle_settings_video_toggle_no_caption(
    bot: &Bot,
    callback_id: &CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    let _ = bot.answer_callback_query(callback_id.clone()).await;
    let current_value = shared_storage
        .get_user_video_no_caption(chat_id.0)
        .await
        .unwrap_or(false);
    let new_value = !current_value;

    shared_storage
        .set_user_video_no_caption(chat_id.0, new_value)
        .await
        .map_err(db_err)?;

    log::info!(
        "User {} toggled video_no_caption: {} -> {}",
        chat_id.0,
        current_value,
        new_value
    );

    show_video_quality_menu(
        bot,
        chat_id,
        message_id,
        Arc::clone(&db_pool),
        Arc::clone(&shared_storage),
        None,
    )
    .await?;
    Ok(())
}

/// Handles `bitrate:` callbacks — persists the chosen audio bitrate and
/// re-renders the bitrate menu.
#[allow(clippy::too_many_arguments)]
async fn handle_settings_bitrate(
    bot: &Bot,
    callback_id: &CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    bitrate: &str,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    let _ = bot.answer_callback_query(callback_id.clone()).await;
    const VALID_BITRATES: &[&str] = &["128k", "192k", "256k", "320k"];
    if !VALID_BITRATES.contains(&bitrate) {
        log::warn!("Rejected invalid bitrate value from user {}: {:?}", chat_id.0, bitrate);
        return Ok(());
    }
    shared_storage
        .set_user_audio_bitrate(chat_id.0, bitrate)
        .await
        .map_err(db_err)?;

    show_audio_bitrate_menu(
        bot,
        chat_id,
        message_id,
        Arc::clone(&db_pool),
        Arc::clone(&shared_storage),
        None,
    )
    .await?;
    Ok(())
}

/// Handles `audio_send_type:toggle` — toggles "send audio as audio/document"
/// and re-renders the bitrate menu.
async fn handle_settings_audio_send_type_toggle(
    bot: &Bot,
    callback_id: &CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    let _ = bot.answer_callback_query(callback_id.clone()).await;
    let current_value = shared_storage
        .get_user_send_audio_as_document(chat_id.0)
        .await
        .unwrap_or(0);
    let new_value = if current_value == 0 { 1 } else { 0 };

    shared_storage
        .set_user_send_audio_as_document(chat_id.0, new_value)
        .await
        .map_err(db_err)?;

    show_audio_bitrate_menu(
        bot,
        chat_id,
        message_id,
        Arc::clone(&db_pool),
        Arc::clone(&shared_storage),
        None,
    )
    .await?;
    Ok(())
}

/// Handles `subtitle:` callbacks — cycles the subtitle style attribute
/// (font_size, text_color, outline_color, outline_width, shadow, position)
/// and re-renders the subtitle style menu.
#[allow(clippy::too_many_arguments)]
async fn handle_settings_subtitle(
    bot: &Bot,
    callback_id: &CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    setting: &str,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    let _ = bot.answer_callback_query(callback_id.clone()).await;
    let style = shared_storage
        .get_user_subtitle_style(chat_id.0)
        .await
        .unwrap_or_default();

    match setting {
        "font_size" => {
            let next = match style.font_size.as_str() {
                "small" => "medium",
                "medium" => "large",
                "large" => "xlarge",
                _ => "small",
            };
            shared_storage
                .set_user_subtitle_font_size(chat_id.0, next)
                .await
                .map_err(db_err)?;
        }
        "text_color" => {
            let next = match style.text_color.as_str() {
                "white" => "yellow",
                "yellow" => "cyan",
                "cyan" => "green",
                _ => "white",
            };
            shared_storage
                .set_user_subtitle_text_color(chat_id.0, next)
                .await
                .map_err(db_err)?;
        }
        "outline_color" => {
            let next = match style.outline_color.as_str() {
                "black" => "dark_gray",
                "dark_gray" => "none",
                _ => "black",
            };
            shared_storage
                .set_user_subtitle_outline_color(chat_id.0, next)
                .await
                .map_err(db_err)?;
        }
        "outline_width" => {
            let next = match style.outline_width {
                0 => 1,
                1 => 2,
                2 => 3,
                3 => 4,
                _ => 0,
            };
            shared_storage
                .set_user_subtitle_outline_width(chat_id.0, next)
                .await
                .map_err(db_err)?;
        }
        "shadow" => {
            let next = match style.shadow {
                0 => 1,
                1 => 2,
                _ => 0,
            };
            shared_storage
                .set_user_subtitle_shadow(chat_id.0, next)
                .await
                .map_err(db_err)?;
        }
        "position" => {
            let next = match style.position.as_str() {
                "bottom" => "top",
                _ => "bottom",
            };
            shared_storage
                .set_user_subtitle_position(chat_id.0, next)
                .await
                .map_err(db_err)?;
        }
        _ => {}
    }

    show_subtitle_style_menu(
        bot,
        chat_id,
        message_id,
        Arc::clone(&db_pool),
        Arc::clone(&shared_storage),
    )
    .await?;
    Ok(())
}

/// Handles `pbar_style:` callbacks — persists the chosen progress-bar style
/// and re-renders the progress-bar menu.
#[allow(clippy::too_many_arguments)]
async fn handle_settings_pbar_style(
    bot: &Bot,
    callback_id: &CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    style_name: &str,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    let _ = bot.answer_callback_query(callback_id.clone()).await;
    const VALID_PBAR_STYLES: &[&str] = &["classic", "gradient", "emoji", "dots", "runner", "rpg", "fire", "moon"];
    if !VALID_PBAR_STYLES.contains(&style_name) {
        log::warn!(
            "Rejected invalid pbar_style value from user {}: {:?}",
            chat_id.0,
            style_name
        );
        return Ok(());
    }
    shared_storage
        .set_user_progress_bar_style(chat_id.0, style_name)
        .await
        .map_err(db_err)?;

    log::info!("User {} set progress bar style to {}", chat_id.0, style_name);

    show_progress_bar_style_menu(
        bot,
        chat_id,
        message_id,
        Arc::clone(&db_pool),
        Arc::clone(&shared_storage),
    )
    .await?;
    Ok(())
}

/// Handles `video_send_type:toggle:` callbacks — toggles the user's send-as
/// preference in-place and rewrites the inline keyboard label on the live
/// preview message without re-rendering the whole menu.
#[allow(clippy::too_many_arguments)]
async fn handle_settings_video_send_type_toggle(
    bot: &Bot,
    callback_id: &CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    data: &str,
    q_message: Option<&teloxide::types::MaybeInaccessibleMessage>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    let _ = bot.answer_callback_query(callback_id.clone()).await;

    let parts: Vec<&str> = data.split(':').collect();
    if parts.len() >= 3 {
        let url_id = parts[2];

        let current_value = shared_storage.get_user_send_as_document(chat_id.0).await.unwrap_or(0);
        let new_value = if current_value == 0 { 1 } else { 0 };

        log::info!(
            "🔄 Video send type toggled for user {}: {} -> {} ({})",
            chat_id.0,
            if current_value == 0 { "Media" } else { "Document" },
            if new_value == 0 { "Media" } else { "Document" },
            if new_value == 0 { "send_video" } else { "send_document" }
        );

        shared_storage
            .set_user_send_as_document(chat_id.0, new_value)
            .await
            .map_err(db_err)?;

        if let Some(teloxide::types::MaybeInaccessibleMessage::Regular(regular_msg)) = q_message {
            if let Some(keyboard) = regular_msg.reply_markup() {
                let mut new_buttons = keyboard.inline_keyboard.clone();

                for row in &mut new_buttons {
                    for button in row {
                        if let teloxide::types::InlineKeyboardButtonKind::CallbackData(ref cb_data) = button.kind {
                            if cb_data.starts_with("video_send_type:toggle:") {
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
    Ok(())
}

/// Handles `back:` callbacks — back navigation with three shapes:
/// `back:preview:url_id[:preview_msg_id]` (rebuild preview card),
/// `back:main:preview:url_id[:preview_msg_id]` (main menu over a preview),
/// and plain `back:main` / `back:enhanced_main` / `back:start`.
#[allow(clippy::too_many_arguments)]
async fn handle_settings_back(
    bot: &Bot,
    callback_id: &CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
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

        match cache::get_url(&db_pool, Some(shared_storage.as_ref()), url_id).await {
            Some(url_str) => match url::Url::parse(&url_str) {
                Ok(url) => {
                    let current_format = shared_storage
                        .get_user_download_format(chat_id.0)
                        .await
                        .unwrap_or_else(|_| "mp3".to_string());
                    let video_quality = if current_format == "mp4" {
                        shared_storage.get_user_video_quality(chat_id.0).await.ok()
                    } else {
                        None
                    };

                    // Experimental features graduated to main workflow
                    match crate::telegram::preview::get_preview_metadata(
                        &url,
                        Some(&current_format),
                        video_quality.as_deref(),
                    )
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
                                    log::error!("Failed to update preview message: {:?}", e);
                                    let _ = bot
                                        .send_message(chat_id, "Failed to update preview. Please send the link again.")
                                        .await;
                                }
                            }
                        }
                        Err(e) => {
                            log::error!("Failed to get preview metadata: {:?}", e);
                            let _ = bot
                                .send_message(chat_id, "Failed to update preview. Please send the link again.")
                                .await;
                        }
                    }
                }
                Err(e) => {
                    log::error!("Failed to parse URL from cache: {}", e);
                    bot.answer_callback_query(callback_id.clone())
                        .text("Error: invalid link")
                        .await?;
                }
            },
            None => {
                log::warn!("URL not found in cache for ID: {} (expired or invalid)", url_id);
                bot.answer_callback_query(callback_id.clone())
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
            bot,
            chat_id,
            message_id,
            Arc::clone(&db_pool),
            Arc::clone(&shared_storage),
            Some(url_id),
            preview_msg_id,
        )
        .await?;
    } else {
        match data {
            "back:main" => {
                edit_main_menu(
                    bot,
                    chat_id,
                    message_id,
                    Arc::clone(&db_pool),
                    Arc::clone(&shared_storage),
                    None,
                    None,
                )
                .await?;
            }
            "back:enhanced_main" => {
                edit_enhanced_main_menu(
                    bot,
                    chat_id,
                    message_id,
                    Arc::clone(&db_pool),
                    Arc::clone(&shared_storage),
                )
                .await?;
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
    Ok(())
}

fn db_err(e: impl std::fmt::Display) -> RequestError {
    RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string())))
}
