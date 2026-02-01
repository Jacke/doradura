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
    let deps_diagnose_cookies = deps.clone();
    let deps_ytdlp = deps.clone();
    let deps_commands = deps.clone();
    let deps_media_upload = deps.clone();
    let deps_messages = deps.clone();
    let deps_precheckout = deps.clone();
    let deps_callback = deps.clone();
    let deps_browser_login = deps.clone();
    let deps_browser_status = deps.clone();

    dptree::entry()
        // Web App Data handler must run FIRST to process Mini App data
        .branch(webapp_handler(deps_webapp))
        // Successful payment handler must be SECOND
        .branch(successful_payment_handler(deps_payment))
        // Hidden admin commands (not in Command enum)
        .branch(update_cookies_handler(deps_cookies))
        .branch(diagnose_cookies_handler(deps_diagnose_cookies))
        .branch(update_ytdlp_handler(deps_ytdlp))
        .branch(browser_login_handler(deps_browser_login))
        .branch(browser_status_handler(deps_browser_status))
        // Command handler
        .branch(command_handler(deps_commands))
        // Media upload handler for premium/vip users
        .branch(media_upload_handler(deps_media_upload))
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

/// Handler for /diagnose_cookies admin command (hidden, not in Command enum)
fn diagnose_cookies_handler(_deps: HandlerDeps) -> UpdateHandler<HandlerError> {
    Update::filter_message()
        .filter(|msg: Message| {
            msg.text()
                .map(|text| text.starts_with("/diagnose_cookies"))
                .unwrap_or(false)
        })
        .endpoint(move |bot: Bot, msg: Message| async move {
            use crate::telegram::admin::handle_diagnose_cookies_command;

            log::info!("🎯 /diagnose_cookies handler matched");
            let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);

            if let Err(e) = handle_diagnose_cookies_command(&bot, msg.chat.id, user_id).await {
                log::error!("❌ /diagnose_cookies handler failed for user {}: {}", user_id, e);
                let _ = bot
                    .send_message(msg.chat.id, format!("❌ /diagnose_cookies failed: {}", e))
                    .await;
            }
            Ok(())
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

/// Handler for /browser_login admin command (hidden, not in Command enum)
fn browser_login_handler(deps: HandlerDeps) -> UpdateHandler<HandlerError> {
    Update::filter_message()
        .filter(|msg: Message| {
            msg.text()
                .map(|text| text.starts_with("/browser_login"))
                .unwrap_or(false)
        })
        .endpoint(move |bot: Bot, msg: Message| {
            let _deps = deps.clone();
            async move {
                use crate::telegram::handle_browser_login_command;

                let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);

                if let Err(e) = handle_browser_login_command(&bot, msg.chat.id, user_id).await {
                    log::error!("❌ /browser_login handler failed for user {}: {}", user_id, e);
                    let _ = bot
                        .send_message(msg.chat.id, format!("❌ /browser_login failed: {}", e))
                        .await;
                }
                Ok(())
            }
        })
}

/// Handler for /browser_status admin command (hidden, not in Command enum)
fn browser_status_handler(deps: HandlerDeps) -> UpdateHandler<HandlerError> {
    Update::filter_message()
        .filter(|msg: Message| {
            msg.text()
                .map(|text| text.starts_with("/browser_status"))
                .unwrap_or(false)
        })
        .endpoint(move |bot: Bot, msg: Message| {
            let _deps = deps.clone();
            async move {
                use crate::telegram::handle_browser_status_command;

                let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);

                if let Err(e) = handle_browser_status_command(&bot, msg.chat.id, user_id).await {
                    log::error!("❌ /browser_status handler failed for user {}: {}", user_id, e);
                    let _ = bot
                        .send_message(msg.chat.id, format!("❌ /browser_status failed: {}", e))
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
                    Command::Uploads => {
                        handle_uploads_command(&bot, &msg, &deps).await?;
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

/// Handle /uploads command
async fn handle_uploads_command(bot: &Bot, msg: &Message, deps: &HandlerDeps) -> Result<(), HandlerError> {
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

/// Handler for media uploads (photo/video/document) from premium/vip users
fn media_upload_handler(deps: HandlerDeps) -> UpdateHandler<HandlerError> {
    use crate::core::subscription::PlanLimits;
    use crate::storage::uploads::{find_duplicate_upload, save_upload, NewUpload};
    use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, ParseMode};

    let deps_filter = deps.clone();

    Update::filter_message()
        .filter(|msg: Message| {
            // Only handle messages with media (photo, video, document, audio)
            msg.photo().is_some() || msg.video().is_some() || msg.document().is_some() || msg.audio().is_some()
        })
        .filter(move |msg: Message| {
            // Skip if user has active cookies upload session (let message_handler process it)
            let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
            if let Ok(conn) = get_connection(&deps_filter.db_pool) {
                if let Ok(Some(_)) = db::get_active_cookies_upload_session(&conn, user_id) {
                    log::info!(
                        "📤 Filter: skipping media_upload_handler - user {} has active cookies session",
                        user_id
                    );
                    return false; // Don't handle - let it fall through to message_handler
                }
            }
            true // Handle this message
        })
        .endpoint(move |bot: Bot, msg: Message| {
            let deps = deps.clone();
            async move {
                let chat_id = msg.chat.id;

                // Get user and check plan
                let conn = match get_connection(&deps.db_pool) {
                    Ok(c) => c,
                    Err(e) => {
                        log::error!("Failed to get DB connection: {}", e);
                        return Ok(());
                    }
                };

                let user = match get_user(&conn, chat_id.0) {
                    Ok(Some(u)) => u,
                    Ok(None) => {
                        // User doesn't exist, create them
                        let username = msg.from.as_ref().and_then(|u| u.username.clone());
                        if let Err(e) = create_user(&conn, chat_id.0, username) {
                            log::error!("Failed to create user: {}", e);
                            return Ok(());
                        }

                        // Fetch the newly created user
                        match get_user(&conn, chat_id.0) {
                            Ok(Some(u)) => u,
                            _ => {
                                log::error!("Failed to get created user");
                                return Ok(());
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to get user: {}", e);
                        return Ok(());
                    }
                };

                // Check if user can upload media
                let limits = PlanLimits::for_plan(&user.plan);
                if !limits.can_upload_media {
                    // Notify user that they can't upload media
                    bot.send_message(
                        chat_id,
                        "❌ Твой тарифный план не позволяет загружать файлы.\n\nИспользуй /plan, чтобы узнать подробнее о тарифах."
                    )
                    .await?;
                    return Ok(());
                }

                // Extract file info from the message
                #[allow(clippy::type_complexity)]
                let (
                    media_type,
                    file_id,
                    file_unique_id,
                    file_size,
                    duration,
                    width,
                    height,
                    mime_type,
                    filename,
                    thumbnail_file_id,
                ): (
                    &str,
                    String,
                    Option<String>,
                    Option<i64>,
                    Option<i64>,
                    Option<i32>,
                    Option<i32>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                ) = if let Some(photos) = msg.photo() {
                    // Get the largest photo
                    let photo = photos.iter().max_by_key(|p| p.width * p.height);
                    if let Some(p) = photo {
                        (
                            "photo",
                            p.file.id.0.clone(),
                            Some(p.file.unique_id.0.clone()),
                            Some(p.file.size as i64),
                            None,
                            Some(p.width as i32),
                            Some(p.height as i32),
                            Some("image/jpeg".to_string()),
                            None,
                            None,
                        )
                    } else {
                        return Ok(());
                    }
                } else if let Some(video) = msg.video() {
                    (
                        "video",
                        video.file.id.0.clone(),
                        Some(video.file.unique_id.0.clone()),
                        Some(video.file.size as i64),
                        Some(video.duration.seconds() as i64),
                        Some(video.width as i32),
                        Some(video.height as i32),
                        video.mime_type.as_ref().map(|m| m.to_string()),
                        video.file_name.clone(),
                        video.thumbnail.as_ref().map(|t| t.file.id.0.clone()),
                    )
                } else if let Some(doc) = msg.document() {
                    (
                        "document",
                        doc.file.id.0.clone(),
                        Some(doc.file.unique_id.0.clone()),
                        Some(doc.file.size as i64),
                        None,
                        None,
                        None,
                        doc.mime_type.as_ref().map(|m| m.to_string()),
                        doc.file_name.clone(),
                        doc.thumbnail.as_ref().map(|t| t.file.id.0.clone()),
                    )
                } else if let Some(audio) = msg.audio() {
                    (
                        "audio",
                        audio.file.id.0.clone(),
                        Some(audio.file.unique_id.0.clone()),
                        Some(audio.file.size as i64),
                        Some(audio.duration.seconds() as i64),
                        None,
                        None,
                        audio.mime_type.as_ref().map(|m| m.to_string()),
                        audio.file_name.clone(),
                        audio.thumbnail.as_ref().map(|t| t.file.id.0.clone()),
                    )
                } else {
                    return Ok(());
                };

                // Check file size limit
                if let Some(size) = file_size {
                    let max_size_bytes = (limits.max_file_size_mb as i64) * 1024 * 1024;
                    if size > max_size_bytes {
                        bot.send_message(
                            chat_id,
                            format!(
                                "❌ Файл слишком большой ({} MB). Максимальный размер для твоего плана: {} MB.",
                                size / 1024 / 1024,
                                limits.max_file_size_mb
                            ),
                        )
                        .await?;
                        return Ok(());
                    }
                }

                // Check for duplicates
                if let Some(ref unique_id) = file_unique_id {
                    if let Ok(Some(existing)) = find_duplicate_upload(&conn, chat_id.0, unique_id) {
                        bot.send_message(
                            chat_id,
                            format!(
                                "ℹ️ Этот файл уже загружен: *{}*\n\nИспользуй /videos чтобы найти его.",
                                crate::core::escape_markdown(&existing.title)
                            ),
                        )
                        .parse_mode(ParseMode::MarkdownV2)
                        .await?;
                        return Ok(());
                    }
                }

                // Extract file format from mime type or filename
                let file_format = mime_type
                    .as_ref()
                    .and_then(|m| m.split('/').next_back().map(|s| s.to_string()))
                    .or_else(|| {
                        filename
                            .as_ref()
                            .and_then(|f| f.rsplit('.').next().map(|s| s.to_lowercase()))
                    });

                // Generate title from filename or default
                let title = filename
                    .as_ref()
                    .map(|f| {
                        // Remove extension from filename
                        f.rsplit_once('.')
                            .map(|(name, _)| name.to_string())
                            .unwrap_or_else(|| f.clone())
                    })
                    .unwrap_or_else(|| {
                        format!(
                            "{} {}",
                            match media_type {
                                "photo" => "Фото",
                                "video" => "Видео",
                                "audio" => "Аудио",
                                _ => "Документ",
                            },
                            chrono::Utc::now().format("%d.%m.%Y %H:%M")
                        )
                    });

                // Save upload to database
                let upload = NewUpload {
                    user_id: chat_id.0,
                    original_filename: filename.as_deref(),
                    title: &title,
                    media_type,
                    file_format: file_format.as_deref(),
                    file_id: &file_id,
                    file_unique_id: file_unique_id.as_deref(),
                    file_size,
                    duration,
                    width,
                    height,
                    mime_type: mime_type.as_deref(),
                    message_id: Some(msg.id.0),
                    chat_id: Some(chat_id.0),
                    thumbnail_file_id: thumbnail_file_id.as_deref(),
                };

                match save_upload(&conn, &upload) {
                    Ok(upload_id) => {
                        log::info!(
                            "Upload saved: id={}, user={}, type={}, title={}",
                            upload_id,
                            chat_id.0,
                            media_type,
                            title
                        );

                        // Format file info for display
                        let size_str = file_size
                            .map(|s| {
                                if s < 1024 * 1024 {
                                    format!("{:.1} KB", s as f64 / 1024.0)
                                } else {
                                    format!("{:.1} MB", s as f64 / 1024.0 / 1024.0)
                                }
                            })
                            .unwrap_or_else(|| "—".to_string());

                        let duration_str = duration.map(|d| {
                            let mins = d / 60;
                            let secs = d % 60;
                            format!("{}:{:02}", mins, secs)
                        });

                        let media_icon = match media_type {
                            "photo" => "📷",
                            "video" => "🎬",
                            "audio" => "🎵",
                            _ => "📄",
                        };

                        let mut info_parts = vec![size_str];
                        if let Some(dur) = duration_str {
                            info_parts.push(dur);
                        }
                        if let Some(w) = width {
                            if let Some(h) = height {
                                info_parts.push(format!("{}x{}", w, h));
                            }
                        }

                        let escaped_title = crate::core::escape_markdown(&title);
                        let escaped_info = crate::core::escape_markdown(&info_parts.join(" · "));

                        // Build action keyboard
                        let keyboard = InlineKeyboardMarkup::new(vec![
                            vec![
                                InlineKeyboardButton::callback("📤 Отправить", format!("videos:send:{}", upload_id)),
                                InlineKeyboardButton::callback("🗑️ Удалить", format!("videos:delete:{}", upload_id)),
                            ],
                            vec![InlineKeyboardButton::callback(
                                "📂 Все загрузки",
                                "videos:page:0:all".to_string(),
                            )],
                        ]);

                        bot.send_message(
                            chat_id,
                            format!(
                                "{} *Файл загружен:* {}\n└ {}\n\nИспользуй /videos чтобы конвертировать файлы\\.",
                                media_icon, escaped_title, escaped_info
                            ),
                        )
                        .parse_mode(ParseMode::MarkdownV2)
                        .reply_markup(keyboard)
                        .await?;
                    }
                    Err(e) => {
                        log::error!("Failed to save upload: {}", e);
                        bot.send_message(chat_id, "❌ Не удалось сохранить файл. Попробуй ещё раз.")
                            .await?;
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
