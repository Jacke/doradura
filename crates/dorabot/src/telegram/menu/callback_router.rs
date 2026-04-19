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

/// Forwards to a sub-handler and log-on-error. Collapses the boilerplate
/// `if let Err(e) = fut.await { log::error!("<label> error: {}", e); }`
/// pattern into one line per dispatch arm.
macro_rules! try_forward {
    ($label:literal, $fut:expr) => {
        if let Err(e) = $fut.await {
            log::error!(concat!($label, " callback error: {}"), e);
        }
    };
}

/// Clones parts of the caller's `CallbackQuery` into a fresh instance
/// suitable for forwarding to a Shape-1 sub-handler (`handle_lyrics_callback`,
/// `handle_audio_cut_callback`, `handle_audio_effects_callback`).
///
/// The outer `if let Some(data) = q.data { ... }` partially moves `q.data`,
/// so we cannot take `&q` inside the match body. Instead the caller passes the
/// already-cloned `message`/`data` plus the still-borrowable `from`,
/// `inline_message_id`, `chat_instance`, `game_short_name`.
fn build_forwarded_query(
    callback_id: &teloxide::types::CallbackQueryId,
    from: &teloxide::types::User,
    message: Option<teloxide::types::MaybeInaccessibleMessage>,
    inline_message_id: Option<&str>,
    chat_instance: &str,
    data: Option<String>,
    game_short_name: Option<&str>,
) -> CallbackQuery {
    CallbackQuery {
        id: callback_id.clone(),
        from: from.clone(),
        message,
        inline_message_id: inline_message_id.map(ToOwned::to_owned),
        chat_instance: chat_instance.to_owned(),
        data,
        game_short_name: game_short_name.map(ToOwned::to_owned),
    }
}

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

            // --- Per-prefix dispatch ---
            let Some(kind) = kind else { return Ok(()) };

            // Partial-move note: `q.data` was moved into `data` above, so we
            // pass the already-cloned `message_clone`/`data_clone` plus the
            // still-accessible individual `q.*` fields to the helper.
            let build_fq =
                |msg: Option<teloxide::types::MaybeInaccessibleMessage>, d: Option<String>| -> CallbackQuery {
                    build_forwarded_query(
                        &callback_id,
                        &q.from,
                        msg,
                        q.inline_message_id.as_deref(),
                        &q.chat_instance,
                        d,
                        q.game_short_name.as_deref(),
                    )
                };

            match kind {
                // --- Shape 1: forward CallbackQuery to sub-handler ---
                CallbackKind::Lyr => {
                    let fq = build_fq(message_clone.clone(), data_clone.clone());
                    try_forward!(
                        "Lyrics",
                        handle_lyrics_callback(bot.clone(), fq, Arc::clone(&shared_storage))
                    );
                }
                CallbackKind::Ac => {
                    let fq = build_fq(message_clone.clone(), data_clone.clone());
                    try_forward!(
                        "Audio cut",
                        handle_audio_cut_callback(bot.clone(), fq, Arc::clone(&shared_storage))
                    );
                }
                CallbackKind::Ae => {
                    let fq = build_fq(message_clone.clone(), data_clone.clone());
                    try_forward!(
                        "Audio effects",
                        handle_audio_effects_callback(bot.clone(), fq, Arc::clone(&shared_storage))
                    );
                }

                // --- Carousel toggle (inline logic) ---
                CallbackKind::Ct => {
                    handle_ct_toggle(&bot, &callback_id, chat_id, message_id, &data, message_clone.as_ref()).await;
                }

                // --- Instagram (ig:sub: handled first as sub-case, then generic ig:) ---
                CallbackKind::Ig => {
                    if let Some(username) = data.strip_prefix("ig:sub:") {
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
                    } else {
                        try_forward!(
                            "Instagram",
                            crate::telegram::instagram::handle_instagram_callback(
                                &bot,
                                &callback_id,
                                chat_id,
                                &data,
                                Arc::clone(&db_pool),
                                Arc::clone(&shared_storage),
                            )
                        );
                    }
                }

                CallbackKind::Cw => {
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
                }

                CallbackKind::Format => {
                    handle_format_callback(
                        &bot,
                        &callback_id,
                        chat_id,
                        message_id,
                        &data,
                        Arc::clone(&db_pool),
                        Arc::clone(&shared_storage),
                        Arc::clone(&download_queue),
                        Arc::clone(&rate_limiter),
                    )
                    .await?;
                }

                CallbackKind::Dl => {
                    // dl:tl:… and dl:tm:… are inline keyboard toggles; the rest delegates.
                    if data.starts_with("dl:tl:") {
                        handle_dl_tl_toggle(&bot, &callback_id, chat_id, message_id, &data, message_clone.as_ref())
                            .await;
                    } else if data.starts_with("dl:tm:") {
                        handle_dl_tm_toggle(&bot, &callback_id, chat_id, message_id, &data, message_clone.as_ref())
                            .await;
                    } else {
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
                    }
                }

                CallbackKind::Pv => {
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
                }

                CallbackKind::History => {
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
                }

                CallbackKind::Export => {
                    let format = data.strip_prefix("export:").unwrap_or("");
                    let _ = bot.answer_callback_query(callback_id.clone()).await;
                    crate::core::export::handle_export(
                        &bot,
                        chat_id,
                        format,
                        Arc::clone(&db_pool),
                        Arc::clone(&shared_storage),
                    )
                    .await?;
                }

                CallbackKind::Vfx => {
                    let _ = bot.answer_callback_query(callback_id.clone()).await;
                    try_forward!(
                        "Voice effect",
                        crate::telegram::voice_effects::handle_voice_effect_callback(
                            &bot,
                            chat_id,
                            message_id,
                            &data,
                            &db_pool,
                            shared_storage.as_ref(),
                        )
                    );
                }

                CallbackKind::Vp => {
                    let _ = bot.answer_callback_query(callback_id.clone()).await;
                    try_forward!(
                        "Vlipsy preview",
                        crate::telegram::preview::vlipsy::handle_vlipsy_callback(
                            &bot,
                            chat_id,
                            message_id,
                            &data,
                            &db_pool,
                            &shared_storage,
                        )
                    );
                }

                CallbackKind::Sr => {
                    use crate::telegram::menu::search::handle_search_callback;
                    try_forward!(
                        "Search",
                        handle_search_callback(
                            &bot,
                            callback_id.clone(),
                            chat_id,
                            message_id,
                            &data,
                            Arc::clone(&db_pool),
                            Arc::clone(&shared_storage),
                            Arc::clone(&download_queue),
                        )
                    );
                }

                CallbackKind::Pw => {
                    use crate::telegram::menu::player::handle_player_callback;
                    try_forward!(
                        "Player",
                        handle_player_callback(
                            &bot,
                            callback_id.clone(),
                            chat_id,
                            message_id,
                            &data,
                            Arc::clone(&db_pool),
                            Arc::clone(&shared_storage),
                            Arc::clone(&download_queue),
                        )
                    );
                }

                CallbackKind::Pl => {
                    use crate::telegram::menu::playlist::handle_playlist_callback;
                    try_forward!(
                        "Playlist",
                        handle_playlist_callback(
                            &bot,
                            callback_id.clone(),
                            chat_id,
                            message_id,
                            &data,
                            Arc::clone(&db_pool),
                            Arc::clone(&shared_storage),
                        )
                    );
                }

                CallbackKind::Vault => {
                    let _ = bot.answer_callback_query(callback_id.clone()).await;
                    try_forward!(
                        "Vault",
                        crate::telegram::menu::vault::handle_vault_callback(
                            &bot,
                            callback_id.clone(),
                            chat_id,
                            message_id,
                            &data,
                            Arc::clone(&db_pool),
                            Arc::clone(&shared_storage),
                        )
                    );
                }

                CallbackKind::Pi => {
                    use crate::telegram::menu::playlist_integrations::handle_playlist_integrations_callback;
                    try_forward!(
                        "Playlist integrations",
                        handle_playlist_integrations_callback(
                            &bot,
                            callback_id.clone(),
                            chat_id,
                            message_id,
                            &data,
                            Arc::clone(&db_pool),
                            Arc::clone(&shared_storage),
                            Arc::clone(&download_queue),
                        )
                    );
                }

                CallbackKind::Vl => {
                    use crate::telegram::menu::vlipsy::handle_vlipsy_callback;
                    try_forward!(
                        "Vlipsy",
                        handle_vlipsy_callback(
                            &bot,
                            callback_id.clone(),
                            chat_id,
                            message_id,
                            &data,
                            Arc::clone(&db_pool),
                            Arc::clone(&shared_storage),
                        )
                    );
                }

                CallbackKind::Ringtone => {
                    use crate::telegram::menu::ringtone::handle_ringtone_callback;
                    try_forward!(
                        "Ringtone",
                        handle_ringtone_callback(
                            &bot,
                            callback_id.clone(),
                            chat_id,
                            message_id,
                            &data,
                            Arc::clone(&db_pool),
                            Arc::clone(&shared_storage),
                        )
                    );
                }

                CallbackKind::Downloads => {
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
                }

                CallbackKind::Cuts => {
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
                }

                CallbackKind::Videos | CallbackKind::Convert => {
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

                // Admin/settings kinds are already handled above; reaching here means
                // the group handler returned false (no match) — silently drop.
                CallbackKind::Analytics
                | CallbackKind::Metrics
                | CallbackKind::Au
                | CallbackKind::Admin
                | CallbackKind::Mode
                | CallbackKind::Main
                | CallbackKind::Ext
                | CallbackKind::Subscribe
                | CallbackKind::Subscription
                | CallbackKind::Language
                | CallbackKind::Quality
                | CallbackKind::SendType
                | CallbackKind::Video
                | CallbackKind::Bitrate
                | CallbackKind::AudioSendType
                | CallbackKind::Subtitle
                | CallbackKind::PbarStyle
                | CallbackKind::VideoSendType
                | CallbackKind::Settings
                | CallbackKind::Back => {}
            }
        }
    }

    Ok(())
}

/// Lyrics toggle: flips MP3 button callbacks between `dl:mp3:` and `dl:mp3+lyr:`.
async fn handle_dl_tl_toggle(
    bot: &Bot,
    callback_id: &teloxide::types::CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    data: &str,
    message: Option<&teloxide::types::MaybeInaccessibleMessage>,
) {
    let _ = bot.answer_callback_query(callback_id.clone()).await;
    let parts: Vec<&str> = data.split(':').collect();
    if parts.len() < 3 {
        return;
    }
    let url_id = parts[2];
    let Some(teloxide::types::MaybeInaccessibleMessage::Regular(regular_msg)) = message else {
        return;
    };
    let Some(keyboard) = regular_msg.reply_markup() else {
        return;
    };
    let mut new_buttons = keyboard.inline_keyboard.clone();

    let currently_on = new_buttons.iter().flatten().any(|btn| {
        matches!(&btn.kind,
            teloxide::types::InlineKeyboardButtonKind::CallbackData(d)
            if d.starts_with("dl:mp3+lyr:"))
    });

    for row in &mut new_buttons {
        for button in row {
            if let teloxide::types::InlineKeyboardButtonKind::CallbackData(ref mut cb) = button.kind {
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
            if let teloxide::types::InlineKeyboardButtonKind::CallbackData(ref cb) = button.kind {
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
    let _ = crate::telegram::styled::edit_message_reply_markup_styled(bot, chat_id, message_id, &new_keyboard).await;
    log::info!(
        "Lyrics toggle: {} → {} for user {}",
        if currently_on { "ON" } else { "OFF" },
        if currently_on { "OFF" } else { "ON" },
        chat_id.0
    );
}

/// MP3 toggle: flips quality-button callbacks between `dl:mp4+mp3:q:uid` and `dl:mp4:q:uid`.
async fn handle_dl_tm_toggle(
    bot: &Bot,
    callback_id: &teloxide::types::CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    data: &str,
    message: Option<&teloxide::types::MaybeInaccessibleMessage>,
) {
    let _ = bot.answer_callback_query(callback_id.clone()).await;
    let parts: Vec<&str> = data.split(':').collect();
    if parts.len() < 3 {
        return;
    }
    let url_id = parts[2];
    let Some(teloxide::types::MaybeInaccessibleMessage::Regular(regular_msg)) = message else {
        return;
    };
    let Some(keyboard) = regular_msg.reply_markup() else {
        return;
    };
    let mut new_buttons = keyboard.inline_keyboard.clone();

    let currently_on = new_buttons.iter().flatten().any(|btn| {
        matches!(&btn.kind,
            teloxide::types::InlineKeyboardButtonKind::CallbackData(d)
            if d.starts_with("dl:mp4+mp3:") && d.split(':').count() == 4)
    });

    for row in &mut new_buttons {
        for button in row {
            if let teloxide::types::InlineKeyboardButtonKind::CallbackData(ref mut cb) = button.kind {
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
            if let teloxide::types::InlineKeyboardButtonKind::CallbackData(ref cb) = button.kind {
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
    let _ = crate::telegram::styled::edit_message_reply_markup_styled(bot, chat_id, message_id, &new_keyboard).await;
    log::info!(
        "MP3 toggle: {} → {} for user {}",
        if currently_on { "ON" } else { "OFF" },
        if currently_on { "OFF" } else { "ON" },
        chat_id.0
    );
}

/// Carousel toggle: `ct:{index}:{url_id}:{mask}` or `ct:all:{url_id}:{mask}`.
async fn handle_ct_toggle(
    bot: &Bot,
    callback_id: &teloxide::types::CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    data: &str,
    message: Option<&teloxide::types::MaybeInaccessibleMessage>,
) {
    let _ = bot.answer_callback_query(callback_id.clone()).await;
    let parts: Vec<&str> = data.splitn(4, ':').collect();
    if parts.len() != 4 {
        return;
    }
    let url_id = parts[2];
    let Ok(mask) = parts[3].parse::<u32>() else {
        return;
    };
    let carousel_count = message
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
        let new_keyboard = crate::telegram::preview::create_carousel_keyboard(carousel_count, mask, url_id);
        let _ = bot
            .edit_message_reply_markup(chat_id, message_id)
            .reply_markup(new_keyboard)
            .await;
    }
}

/// Handles `format:` callbacks. Three shapes:
/// - `format:mp3` — update user default, then show download-type menu
/// - `format:mp3:preview:{url_id}` — set default and start preview download
/// - `format:mp3:preview:{url_id}:{preview_msg_id}` — same, with preview message id
#[allow(clippy::too_many_arguments)]
async fn handle_format_callback(
    bot: &Bot,
    callback_id: &teloxide::types::CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    download_queue: Arc<DownloadQueue>,
    rate_limiter: Arc<RateLimiter>,
) -> ResponseResult<()> {
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
                bot,
                callback_id,
                chat_id,
                message_id,
                preview_msg_id,
                id,
                format,
                None,
                db_pool,
                shared_storage,
                download_queue,
                rate_limiter,
            )
            .await?;
        }
    } else {
        show_download_type_menu(bot, chat_id, message_id, db_pool, shared_storage, None, None).await?;
    }
    Ok(())
}
