//! Command handler implementations (/start, /downloads, /uploads, /cuts)

use teloxide::types::Message;

use super::types::{HandlerDeps, HandlerError};
use crate::i18n;
use crate::storage::db;
use crate::storage::get_connection;
use crate::telegram::Bot;

/// Handle /start command
pub(super) async fn handle_start_command(bot: &Bot, msg: &Message, deps: &HandlerDeps) -> Result<(), HandlerError> {
    use crate::telegram::{
        send_random_voice_message, setup_chat_bot_commands, show_enhanced_main_menu, show_language_selection_menu,
    };

    // Check if user exists
    let user_exists = if let Ok(conn) = get_connection(&deps.db_pool) {
        let chat_id = msg.chat.id.0;
        matches!(db::get_user(&conn, chat_id), Ok(Some(_)))
    } else {
        false
    };

    if user_exists {
        // Existing user - show enhanced main menu
        let _ = show_enhanced_main_menu(bot, msg.chat.id, deps.db_pool.clone()).await;
        let lang = i18n::user_lang_from_pool(&deps.db_pool, msg.chat.id.0);
        if let Err(e) = setup_chat_bot_commands(bot, msg.chat.id, &lang).await {
            log::warn!("Failed to set chat-specific commands: {}", e);
        }

        // Send random voice message in background
        let bot_voice = bot.clone();
        let chat_id_voice = msg.chat.id;
        tokio::spawn(async move {
            send_random_voice_message(bot_voice, chat_id_voice).await;
        });
    } else {
        // New user - try to auto-detect language from Telegram profile
        let detected_lang = msg
            .from
            .as_ref()
            .and_then(|user| user.language_code.as_deref())
            .and_then(i18n::is_language_supported);

        if let Some(lang_code) = detected_lang {
            // Supported language detected - create user with auto-detected language
            log::info!(
                "New user on /start: chat_id={}, auto-detected language: {}",
                msg.chat.id.0,
                lang_code
            );

            if let Ok(conn) = get_connection(&deps.db_pool) {
                let username = msg.from.as_ref().and_then(|u| u.username.clone());
                if let Err(e) = db::create_user_with_language(&conn, msg.chat.id.0, username.clone(), lang_code) {
                    log::warn!("Failed to create user with auto-detected language: {}", e);
                } else {
                    // Notify admins about new user
                    use crate::telegram::notifications::notify_admin_new_user;
                    let bot_notify = bot.clone();
                    let user_id = msg.chat.id.0;
                    let first_name = msg.from.as_ref().map(|u| u.first_name.clone());
                    let lang = lang_code.to_string();
                    tokio::spawn(async move {
                        notify_admin_new_user(
                            &bot_notify,
                            user_id,
                            username.as_deref(),
                            first_name.as_deref(),
                            Some(&lang),
                            Some("/start"),
                        )
                        .await;
                    });
                }
            }

            // Show enhanced main menu in detected language
            let _ = show_enhanced_main_menu(bot, msg.chat.id, deps.db_pool.clone()).await;
            let lang = i18n::lang_from_code(lang_code);
            if let Err(e) = setup_chat_bot_commands(bot, msg.chat.id, &lang).await {
                log::warn!("Failed to set chat-specific commands: {}", e);
            }

            // Send random voice message in background
            let bot_voice = bot.clone();
            let chat_id_voice = msg.chat.id;
            tokio::spawn(async move {
                send_random_voice_message(bot_voice, chat_id_voice).await;
            });
        } else {
            // No language detected or unsupported - show language selection menu
            log::info!(
                "New user on /start: chat_id={}, no supported language detected, showing language selection",
                msg.chat.id.0
            );
            let _ = show_language_selection_menu(bot, msg.chat.id).await;
        }
    }

    Ok(())
}

/// Handle /downloads command
pub(super) async fn handle_downloads_command(bot: &Bot, msg: &Message, deps: &HandlerDeps) -> Result<(), HandlerError> {
    use crate::telegram::downloads::show_downloads_page;

    log::info!("⚡ Command::Downloads matched");

    let message_text = msg.text().unwrap_or("");
    let args: Vec<&str> = message_text.split_whitespace().collect();

    let (filter, search) = if args.len() > 1 {
        match args[1].to_lowercase().as_str() {
            "mp3" => (Some("mp3".to_string()), None),
            "mp4" => (Some("mp4".to_string()), None),
            _ => {
                let search_query = args[1..].join(" ");
                (None, Some(search_query))
            }
        }
    } else {
        (None, None)
    };

    log::info!(
        "📥 Showing downloads page with filter={:?}, search={:?}",
        filter,
        search
    );

    match show_downloads_page(bot, msg.chat.id, deps.db_pool.clone(), 0, filter, search).await {
        Ok(_) => log::info!("✅ Downloads page shown successfully"),
        Err(e) => log::error!("❌ Failed to show downloads page: {:?}", e),
    }

    Ok(())
}

/// Handle /uploads command
pub(super) async fn handle_uploads_command(bot: &Bot, msg: &Message, deps: &HandlerDeps) -> Result<(), HandlerError> {
    use crate::telegram::videos::show_videos_page;

    log::info!("⚡ Command::Uploads matched");

    let message_text = msg.text().unwrap_or("");
    let args: Vec<&str> = message_text.split_whitespace().collect();

    let (filter, search) = if args.len() > 1 {
        match args[1].to_lowercase().as_str() {
            "video" => (Some("video".to_string()), None),
            "photo" => (Some("photo".to_string()), None),
            "document" => (Some("document".to_string()), None),
            "audio" => (Some("audio".to_string()), None),
            _ => {
                let search_query = args[1..].join(" ");
                (None, Some(search_query))
            }
        }
    } else {
        (None, None)
    };

    log::info!("📂 Showing videos page with filter={:?}, search={:?}", filter, search);

    match show_videos_page(bot, msg.chat.id, deps.db_pool.clone(), 0, filter, search).await {
        Ok(_) => log::info!("✅ Videos page shown successfully"),
        Err(e) => log::error!("❌ Failed to show videos page: {:?}", e),
    }

    Ok(())
}

/// Handle /cuts command
pub(super) async fn handle_cuts_command(bot: &Bot, msg: &Message, deps: &HandlerDeps) -> Result<(), HandlerError> {
    use crate::telegram::cuts::show_cuts_page;

    let message_text = msg.text().unwrap_or("");
    let args: Vec<&str> = message_text.split_whitespace().collect();
    let page = if args.len() > 1 {
        args[1].parse::<usize>().unwrap_or(0)
    } else {
        0
    };

    match show_cuts_page(bot, msg.chat.id, deps.db_pool.clone(), page).await {
        Ok(_) => log::info!("✅ Cuts page shown successfully"),
        Err(e) => log::error!("❌ Failed to show cuts page: {:?}", e),
    }

    Ok(())
}

/// Handle /style command
pub(super) async fn handle_style_command(bot: &Bot, msg: &Message, deps: &HandlerDeps) -> Result<(), HandlerError> {
    use crate::telegram::menu::show_progress_bar_style_menu;
    use teloxide::prelude::Requester;

    let sent = bot
        .send_message(msg.chat.id, "Loading...")
        .await
        .map_err(HandlerError::from)?;
    show_progress_bar_style_menu(bot, msg.chat.id, sent.id, deps.db_pool.clone())
        .await
        .map_err(HandlerError::from)?;
    Ok(())
}
