//! Telegram bot handler tree configuration
//!
//! This module provides the main dispatcher schema for the Telegram bot.
//! The handlers are organized in a testable way, allowing integration tests
//! to use the same handler tree as production code.

use std::sync::Arc;

use teloxide::dispatching::{UpdateFilterExt, UpdateHandler};
use teloxide::prelude::*;
use teloxide::types::Message;

use crate::core::alerts::AlertManager;
use crate::core::rate_limiter::RateLimiter;
use crate::download::queue::{self as queue, DownloadQueue};
use crate::downsub::DownsubGateway;
use crate::i18n;
use crate::storage::db::{self, create_user, get_user};
use crate::storage::get_connection;
use crate::telegram::bot::Command;
use crate::telegram::Bot;

/// Error type for handlers
pub type HandlerError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Dependencies required by handlers
#[derive(Clone)]
pub struct HandlerDeps {
    pub db_pool: Arc<db::DbPool>,
    pub download_queue: Arc<DownloadQueue>,
    pub rate_limiter: Arc<RateLimiter>,
    pub downsub_gateway: Arc<DownsubGateway>,
    pub bot_username: Option<String>,
    pub bot_id: UserId,
    pub alert_manager: Option<Arc<AlertManager>>,
}

impl HandlerDeps {
    /// Create new handler dependencies
    pub fn new(
        db_pool: Arc<db::DbPool>,
        download_queue: Arc<DownloadQueue>,
        rate_limiter: Arc<RateLimiter>,
        downsub_gateway: Arc<DownsubGateway>,
        bot_username: Option<String>,
        bot_id: UserId,
        alert_manager: Option<Arc<AlertManager>>,
    ) -> Self {
        Self {
            db_pool,
            download_queue,
            rate_limiter,
            downsub_gateway,
            bot_username,
            bot_id,
            alert_manager,
        }
    }
}

/// Creates the main dispatcher schema for the Telegram bot.
///
/// This function returns a handler tree that can be used with teloxide's Dispatcher.
/// The same schema is used in production and can be used in integration tests.
///
/// # Arguments
/// * `deps` - Handler dependencies (database pool, download queue, etc.)
///
/// # Returns
/// The complete handler tree for the bot
pub fn schema(deps: HandlerDeps) -> UpdateHandler<HandlerError> {
    let deps_webapp = deps.clone();
    let deps_payment = deps.clone();
    let deps_cookies = deps.clone();
    let deps_ytdlp = deps.clone();
    let deps_commands = deps.clone();
    let deps_messages = deps.clone();
    let deps_precheckout = deps.clone();
    let deps_callback = deps.clone();

    dptree::entry()
        // Web App Data handler must run FIRST to process Mini App data
        .branch(webapp_handler(deps_webapp))
        // Successful payment handler must be SECOND
        .branch(successful_payment_handler(deps_payment))
        // Hidden admin commands (not in Command enum)
        .branch(update_cookies_handler(deps_cookies))
        .branch(update_ytdlp_handler(deps_ytdlp))
        // Command handler
        .branch(command_handler(deps_commands))
        // Message handler for URLs and text
        .branch(message_handler(deps_messages))
        // Pre-checkout query handler
        .branch(pre_checkout_handler(deps_precheckout))
        // Callback query handler
        .branch(callback_handler(deps_callback))
}

/// Handler for Web App data from Telegram Mini Apps
fn webapp_handler(deps: HandlerDeps) -> UpdateHandler<HandlerError> {
    Update::filter_message()
        .filter(|msg: Message| msg.web_app_data().is_some())
        .endpoint(move |bot: Bot, msg: Message| {
            let deps = deps.clone();
            async move {
                use crate::telegram::{WebAppAction, WebAppData};

                log::info!("Received web_app_data message");

                if let Some(web_app_data) = msg.web_app_data() {
                    let data_str = &web_app_data.data;
                    log::debug!("Web App Data: {}", data_str);

                    // Create the user if they don't exist
                    if let Ok(conn) = get_connection(&deps.db_pool) {
                        let chat_id = msg.chat.id.0;
                        if let Ok(None) = get_user(&conn, chat_id) {
                            let username = msg.from.as_ref().and_then(|u| u.username.clone());
                            if create_user(&conn, chat_id, username.clone()).is_ok() {
                                // Notify admins about new user
                                use crate::telegram::notifications::notify_admin_new_user;
                                let bot_notify = bot.clone();
                                let first_name = msg.from.as_ref().map(|u| u.first_name.clone());
                                let lang_code = msg.from.as_ref().and_then(|u| u.language_code.clone());
                                tokio::spawn(async move {
                                    notify_admin_new_user(
                                        &bot_notify,
                                        chat_id,
                                        username.as_deref(),
                                        first_name.as_deref(),
                                        lang_code.as_deref(),
                                        Some("Web App"),
                                    )
                                    .await;
                                });
                            }
                        }
                    }

                    // Try to parse as the new format with an action field
                    if let Ok(action_data) = serde_json::from_str::<WebAppAction>(data_str) {
                        log::info!("Parsed Web App Action: {:?}", action_data);

                        if action_data.action == "upgrade_plan" {
                            if let Some(plan) = action_data.plan {
                                let lang = i18n::user_lang_from_pool(&deps.db_pool, msg.chat.id.0);
                                let plan_name = match plan.as_str() {
                                    "premium" => "Premium",
                                    "vip" => "VIP",
                                    _ => "Unknown",
                                };

                                let mut args = fluent_templates::fluent_bundle::FluentArgs::new();
                                args.set("plan", plan_name);
                                let message = i18n::t_args(&lang, "subscription.upgrade_prompt", &args);

                                let _ = bot
                                    .send_message(msg.chat.id, message)
                                    .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                                    .await;

                                log::info!("User {} requested upgrade to {}", msg.chat.id, plan);
                            }
                        }
                    }
                    // Fall back to legacy WebAppData format
                    else if let Ok(app_data) = serde_json::from_str::<WebAppData>(data_str) {
                        log::info!("Parsed Web App Data (legacy): {:?}", app_data);

                        match url::Url::parse(&app_data.url) {
                            Ok(url) => {
                                let is_video = app_data.format == "mp4";
                                let format = app_data.format.clone();

                                let task = queue::DownloadTask::new(
                                    url.to_string(),
                                    msg.chat.id,
                                    Some(msg.id.0),
                                    is_video,
                                    format,
                                    app_data.video_quality,
                                    app_data.audio_bitrate,
                                );

                                deps.download_queue
                                    .add_task(task, Some(Arc::clone(&deps.db_pool)))
                                    .await;

                                let _ = bot
                                    .send_message(msg.chat.id, "✅ Задача добавлена в очередь! Скоро отправлю файл.")
                                    .await;

                                log::info!("Task from Mini App added to queue for user {}", msg.chat.id);
                            }
                            Err(e) => {
                                log::error!("Invalid URL from Mini App: {}", e);
                                let _ = bot
                                    .send_message(msg.chat.id, "❌ Некорректная ссылка. Попробуй еще раз.")
                                    .await;
                            }
                        }
                    } else {
                        log::error!("Failed to parse Web App Data as any known format");
                        let _ = bot
                            .send_message(msg.chat.id, "❌ Ошибка обработки данных. Попробуй еще раз.")
                            .await;
                    }
                }

                Ok(())
            }
        })
}

/// Handler for successful Telegram payments
fn successful_payment_handler(deps: HandlerDeps) -> UpdateHandler<HandlerError> {
    Update::filter_message()
        .filter(|msg: Message| msg.successful_payment().is_some())
        .endpoint(move |bot: Bot, msg: Message| {
            let deps = deps.clone();
            async move {
                use crate::core::subscription;
                use crate::telegram::notifications::notify_admin_text;

                log::info!("Received successful_payment message");

                if let Err(e) = subscription::handle_successful_payment(&bot, &msg, Arc::clone(&deps.db_pool)).await {
                    log::error!("Failed to handle successful payment: {:?}", e);
                    notify_admin_text(
                        &bot,
                        &format!("PAYMENT HANDLER ERROR\nchat_id: {}\nerror: {:?}", msg.chat.id.0, e),
                    )
                    .await;
                }
                Ok(())
            }
        })
}

/// Handler for /update_cookies admin command (hidden, not in Command enum)
fn update_cookies_handler(deps: HandlerDeps) -> UpdateHandler<HandlerError> {
    Update::filter_message()
        .filter(|msg: Message| {
            msg.text()
                .map(|text| text.starts_with("/update_cookies"))
                .unwrap_or(false)
        })
        .endpoint(move |bot: Bot, msg: Message| {
            let deps = deps.clone();
            async move {
                use crate::telegram::handle_update_cookies_command;

                log::info!("🎯 /update_cookies handler matched - routing to handle_update_cookies_command");
                let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                let message_text = msg.text().unwrap_or_default();

                if let Err(e) =
                    handle_update_cookies_command(deps.db_pool.clone(), &bot, msg.chat.id, user_id, message_text).await
                {
                    log::error!("❌ /update_cookies handler failed for user {}: {}", user_id, e);
                    let _ = bot
                        .send_message(msg.chat.id, format!("❌ /update_cookies failed: {}", e))
                        .await;
                }
                Ok(())
            }
        })
}

/// Handler for /update_ytdlp admin command (hidden, not in Command enum)
fn update_ytdlp_handler(deps: HandlerDeps) -> UpdateHandler<HandlerError> {
    Update::filter_message()
        .filter(|msg: Message| {
            msg.text()
                .map(|text| text.starts_with("/update_ytdlp"))
                .unwrap_or(false)
        })
        .endpoint(move |bot: Bot, msg: Message| {
            let _deps = deps.clone();
            async move {
                use crate::telegram::handle_update_ytdlp_command;

                log::info!("🎯 /update_ytdlp handler matched - routing to handle_update_ytdlp_command");
                let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                let message_text = msg.text().unwrap_or_default();

                if let Err(e) = handle_update_ytdlp_command(&bot, msg.chat.id, user_id, message_text).await {
                    log::error!("❌ /update_ytdlp handler failed for user {}: {}", user_id, e);
                    let _ = bot
                        .send_message(msg.chat.id, format!("❌ /update_ytdlp failed: {}", e))
                        .await;
                }
                Ok(())
            }
        })
}

/// Handler for bot commands (/start, /settings, /info, etc.)
fn command_handler(deps: HandlerDeps) -> UpdateHandler<HandlerError> {
    use crate::core::{
        export::show_export_menu, history::show_history, stats::show_user_stats, subscription::show_subscription_info,
    };
    use crate::telegram::{
        handle_admin_command, handle_analytics_command, handle_backup_command, handle_botapi_speed_command,
        handle_charges_command, handle_download_tg_command, handle_downsub_command, handle_downsub_health_command,
        handle_health_command, handle_info_command, handle_metrics_command, handle_revenue_command,
        handle_sent_files_command, handle_setplan_command, handle_transactions_command, handle_users_command,
        handle_version_command, show_main_menu,
    };

    Update::filter_message().branch(dptree::entry().filter_command::<Command>().endpoint(
        move |bot: Bot, msg: Message, cmd: Command| {
            let deps = deps.clone();
            async move {
                log::info!("🎯 Received command: {:?} from chat {}", cmd, msg.chat.id);

                match cmd {
                    Command::Start => {
                        handle_start_command(&bot, &msg, &deps).await?;
                    }
                    Command::Settings => {
                        let _ = show_main_menu(&bot, msg.chat.id, deps.db_pool.clone()).await;
                    }
                    Command::Info => {
                        log::info!("⚡ Command::Info matched, calling handle_info_command");
                        match handle_info_command(bot.clone(), msg.clone(), deps.db_pool.clone()).await {
                            Ok(_) => log::info!("✅ handle_info_command completed successfully"),
                            Err(e) => log::error!("❌ handle_info_command failed: {:?}", e),
                        }
                    }
                    Command::Downsub => {
                        let _ = handle_downsub_command(
                            bot.clone(),
                            msg.clone(),
                            deps.db_pool.clone(),
                            deps.downsub_gateway.clone(),
                        )
                        .await;
                    }
                    Command::History => {
                        let _ = show_history(&bot, msg.chat.id, deps.db_pool.clone()).await;
                    }
                    Command::Downloads => {
                        handle_downloads_command(&bot, &msg, &deps).await?;
                    }
                    Command::Cuts => {
                        handle_cuts_command(&bot, &msg, &deps).await?;
                    }
                    Command::Stats => {
                        log::info!("Stats command called for user {}", msg.chat.id);
                        match show_user_stats(&bot, msg.chat.id, deps.db_pool.clone()).await {
                            Ok(_) => log::info!("Stats sent successfully"),
                            Err(e) => log::error!("Failed to show user stats: {:?}", e),
                        }
                    }
                    Command::Export => {
                        let _ = show_export_menu(&bot, msg.chat.id, deps.db_pool.clone()).await;
                    }
                    Command::Backup => {
                        let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                        let _ = handle_backup_command(&bot, msg.chat.id, user_id).await;
                    }
                    Command::Plan => {
                        let _ = show_subscription_info(&bot, msg.chat.id, deps.db_pool.clone()).await;
                    }
                    Command::Users => {
                        let username = msg.from.as_ref().and_then(|u| u.username.as_deref());
                        let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                        let _ = handle_users_command(&bot, msg.chat.id, username, user_id, deps.db_pool.clone()).await;
                    }
                    Command::Setplan => {
                        let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                        let message_text = msg.text().unwrap_or("");
                        let _ = handle_setplan_command(&bot, msg.chat.id, user_id, message_text, deps.db_pool.clone())
                            .await;
                    }
                    Command::Transactions => {
                        let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                        let _ = handle_transactions_command(&bot, msg.chat.id, user_id).await;
                    }
                    Command::Admin => {
                        let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                        let _ = handle_admin_command(&bot, msg.chat.id, user_id, deps.db_pool.clone()).await;
                    }
                    Command::Charges => {
                        let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                        let message_text = msg.text().unwrap_or("");
                        let args = message_text.strip_prefix("/charges").unwrap_or("").trim();
                        let _ = handle_charges_command(&bot, msg.chat.id, user_id, deps.db_pool.clone(), args).await;
                    }
                    Command::DownloadTg => {
                        let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                        let username = msg.from.as_ref().and_then(|u| u.username.as_deref());
                        let message_text = msg.text().unwrap_or("");
                        let _ = handle_download_tg_command(&bot, msg.chat.id, user_id, username, message_text).await;
                    }
                    Command::SentFiles => {
                        let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                        let username = msg.from.as_ref().and_then(|u| u.username.as_deref());
                        let message_text = msg.text().unwrap_or("");
                        let _ = handle_sent_files_command(
                            &bot,
                            msg.chat.id,
                            user_id,
                            username,
                            deps.db_pool.clone(),
                            message_text,
                        )
                        .await;
                    }
                    Command::Analytics => {
                        let _ = handle_analytics_command(bot.clone(), msg.clone(), deps.db_pool.clone()).await;
                    }
                    Command::Health => {
                        let _ = handle_health_command(bot.clone(), msg.clone(), deps.db_pool.clone()).await;
                    }
                    Command::DownsubHealth => {
                        let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                        let _ = handle_downsub_health_command(&bot, msg.chat.id, user_id, deps.downsub_gateway.clone())
                            .await;
                    }
                    Command::Metrics => {
                        let _ = handle_metrics_command(bot.clone(), msg.clone(), deps.db_pool.clone(), None).await;
                    }
                    Command::Revenue => {
                        let _ = handle_revenue_command(bot.clone(), msg.clone(), deps.db_pool.clone()).await;
                    }
                    Command::BotApiSpeed => {
                        let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                        let _ = handle_botapi_speed_command(&bot, msg.chat.id, user_id).await;
                    }
                    Command::Version => {
                        let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                        let _ = handle_version_command(&bot, msg.chat.id, user_id).await;
                    }
                }
                Ok(())
            }
        },
    ))
}

/// Handle /start command
async fn handle_start_command(bot: &Bot, msg: &Message, deps: &HandlerDeps) -> Result<(), HandlerError> {
    use crate::telegram::{
        send_random_voice_message, setup_chat_bot_commands, show_enhanced_main_menu, show_language_selection_menu,
    };

    // Check if user exists
    let user_exists = if let Ok(conn) = get_connection(&deps.db_pool) {
        let chat_id = msg.chat.id.0;
        matches!(get_user(&conn, chat_id), Ok(Some(_)))
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
async fn handle_downloads_command(bot: &Bot, msg: &Message, deps: &HandlerDeps) -> Result<(), HandlerError> {
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

/// Handle /cuts command
async fn handle_cuts_command(bot: &Bot, msg: &Message, deps: &HandlerDeps) -> Result<(), HandlerError> {
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

/// Handler for regular messages (URLs, text)
fn message_handler(deps: HandlerDeps) -> UpdateHandler<HandlerError> {
    use crate::storage::db::log_request;
    use crate::telegram::{handle_message, is_message_addressed_to_bot};

    let bot_username = deps.bot_username.clone();
    let bot_id = deps.bot_id;

    Update::filter_message()
        .filter(move |msg: Message| is_message_addressed_to_bot(&msg, bot_username.as_deref(), bot_id))
        .endpoint(move |bot: Bot, msg: Message| {
            let deps = deps.clone();
            async move {
                // Handle message and get user info
                let user_info_result = handle_message(
                    bot.clone(),
                    msg.clone(),
                    deps.download_queue.clone(),
                    deps.rate_limiter.clone(),
                    deps.db_pool.clone(),
                    deps.alert_manager.clone(),
                )
                .await;

                // Log request and manage user
                if let Some(text) = msg.text() {
                    match &user_info_result {
                        Ok(Some(user)) => {
                            if let Ok(conn) = get_connection(&deps.db_pool) {
                                if let Err(e) = log_request(&conn, user.telegram_id(), text) {
                                    log::error!("Failed to log request: {}", e);
                                }
                            }
                        }
                        Ok(None) | Err(_) => {
                            if let Ok(conn) = get_connection(&deps.db_pool) {
                                let chat_id = msg.chat.id.0;
                                match get_user(&conn, chat_id) {
                                    Ok(Some(user)) => {
                                        if let Err(e) = log_request(&conn, user.telegram_id(), text) {
                                            log::error!("Failed to log request: {}", e);
                                        }
                                    }
                                    Ok(None) => {
                                        let username = msg.from.as_ref().and_then(|u| u.username.clone());
                                        if let Err(e) = create_user(&conn, chat_id, username.clone()) {
                                            log::error!("Failed to create user: {}", e);
                                        } else {
                                            if let Err(e) = log_request(&conn, chat_id, text) {
                                                log::error!("Failed to log request for new user: {}", e);
                                            }
                                            // Notify admins about new user
                                            use crate::telegram::notifications::notify_admin_new_user;
                                            let bot_notify = bot.clone();
                                            let first_name = msg.from.as_ref().map(|u| u.first_name.clone());
                                            let lang_code = msg.from.as_ref().and_then(|u| u.language_code.clone());
                                            let first_message = text.to_string();
                                            tokio::spawn(async move {
                                                notify_admin_new_user(
                                                    &bot_notify,
                                                    chat_id,
                                                    username.as_deref(),
                                                    first_name.as_deref(),
                                                    lang_code.as_deref(),
                                                    Some(&first_message),
                                                )
                                                .await;
                                            });
                                        }
                                    }
                                    Err(e) => {
                                        log::error!("Failed to get user from database: {}", e);
                                    }
                                }
                            }
                        }
                    }
                }

                if let Err(err) = user_info_result {
                    log::error!("Error handling message: {:?}", err);
                }

                Ok(())
            }
        })
}

/// Handler for pre-checkout queries (Telegram payments)
fn pre_checkout_handler(deps: HandlerDeps) -> UpdateHandler<HandlerError> {
    Update::filter_pre_checkout_query().endpoint(move |bot: Bot, query: teloxide::types::PreCheckoutQuery| {
        let deps = deps.clone();
        async move {
            let query_id = query.id;
            let payload = query.invoice_payload;
            let user_id = query.from.id.0;

            log::info!("Received pre_checkout_query: id={}, payload={}", query_id, payload);

            // Validate the payload
            if payload.starts_with("subscription:") {
                // Approve the payment
                match bot.answer_pre_checkout_query(query_id.clone(), true).await {
                    Ok(_) => {
                        log::info!("✅ Pre-checkout query approved for payload: {}", payload);
                    }
                    Err(e) => {
                        log::error!("Failed to answer pre_checkout_query: {:?}", e);
                    }
                }
            } else {
                // Reject unknown payment types
                let lang = i18n::user_lang_from_pool(&deps.db_pool, user_id as i64);
                match bot
                    .answer_pre_checkout_query(query_id.clone(), false)
                    .error_message(i18n::t(&lang, "payment.unknown_type"))
                    .await
                {
                    Ok(_) => {
                        log::info!("Pre-checkout query rejected for payload: {}", payload);
                    }
                    Err(e) => {
                        log::error!("Failed to answer pre_checkout_query: {:?}", e);
                    }
                }
            }
            Ok(())
        }
    })
}

/// Handler for callback queries (inline keyboard buttons)
fn callback_handler(deps: HandlerDeps) -> UpdateHandler<HandlerError> {
    use crate::telegram::handle_menu_callback;

    Update::filter_callback_query().endpoint(move |bot: Bot, q: CallbackQuery| {
        let deps = deps.clone();
        async move {
            handle_menu_callback(
                bot,
                q,
                deps.db_pool.clone(),
                deps.download_queue.clone(),
                deps.rate_limiter.clone(),
            )
            .await
            .map_err(|e| Box::new(e) as HandlerError)
        }
    })
}

// Integration tests are in tests/real_handlers_test.rs
