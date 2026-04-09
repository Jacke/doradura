//! Dispatcher schema and handler chain builders

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use teloxide::dispatching::{UpdateFilterExt, UpdateHandler};
use teloxide::prelude::*;
use teloxide::types::{InlineQuery, Message};

use super::commands::{handle_cuts_command, handle_downloads_command, handle_start_command, handle_uploads_command};
use super::types::{HandlerDeps, HandlerError};
use super::uploads::media_upload_handler;
use crate::i18n;
use crate::telegram::bot::Command;
use crate::telegram::Bot;

/// Unix timestamp when the bot started. Messages older than this
/// (minus a grace period) are skipped to avoid flooding users after a restart,
/// while still processing all payment events.
static BOOT_TIMESTAMP: AtomicI64 = AtomicI64::new(0);

/// Grace period in seconds — process messages sent up to this many seconds
/// before boot (covers clock skew and messages in-flight during startup).
const STALE_MESSAGE_GRACE_SECS: i64 = 60;

/// Initialize the boot timestamp. Call once at startup before dispatching.
pub fn init_boot_timestamp() {
    BOOT_TIMESTAMP.store(chrono::Utc::now().timestamp(), Ordering::Relaxed);
}

/// Returns true if the message is fresh enough to process (sent after boot - grace).
/// Used to skip accumulated messages after a restart without losing payments.
fn is_fresh_message(msg: &Message) -> bool {
    let boot = BOOT_TIMESTAMP.load(Ordering::Relaxed);
    if boot == 0 {
        return true; // not initialized yet, process everything
    }
    let cutoff = boot - STALE_MESSAGE_GRACE_SECS;
    let msg_time = msg.date.timestamp();
    if msg_time >= cutoff {
        return true;
    }
    log::debug!(
        "Skipping stale message ({}s old) from chat {}",
        boot - msg_time,
        msg.chat.id
    );
    false
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
    let deps_payment = deps.clone();
    let deps_cookies = deps.clone();
    let deps_ig_cookies = deps.clone();
    let deps_diagnose_cookies = deps.clone();
    let deps_ytdlp = deps.clone();
    let deps_commands = deps.clone();
    let deps_media_upload = deps.clone();
    let deps_voice = deps.clone();
    let deps_messages = deps.clone();
    let deps_precheckout = deps.clone();
    let deps_refunded = deps.clone();
    let deps_callback = deps.clone();
    let deps_browser_login = deps.clone();
    let deps_browser_status = deps.clone();
    let deps_send = deps.clone();
    let deps_broadcast = deps.clone();
    let deps_inline = deps.clone();

    dptree::entry()
        // ── Payment handlers: NEVER skip, regardless of message age ──
        .branch(successful_payment_handler(deps_payment))
        .branch(refunded_payment_handler(deps_refunded))
        .branch(pre_checkout_handler(deps_precheckout))
        // ── All other message handlers: skip stale messages to avoid flood after restart ──
        .branch(update_cookies_handler(deps_cookies))
        .branch(update_ig_cookies_handler(deps_ig_cookies))
        .branch(diagnose_cookies_handler(deps_diagnose_cookies))
        .branch(update_ytdlp_handler(deps_ytdlp))
        .branch(browser_login_handler(deps_browser_login))
        .branch(browser_status_handler(deps_browser_status))
        .branch(send_handler(deps_send))
        .branch(broadcast_handler(deps_broadcast))
        .branch(command_handler(deps_commands))
        .branch(media_upload_handler(deps_media_upload))
        .branch(voice_message_handler(deps_voice))
        .branch(message_handler(deps_messages))
        // Callback query handler
        .branch(callback_handler(deps_callback))
        // Inline query handler (Vlipsy search)
        .branch(inline_query_handler(deps_inline))
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

                if let Err(e) =
                    subscription::handle_successful_payment(&bot, &msg, Arc::clone(&deps.shared_storage)).await
                {
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

/// Handler for refunded Telegram payments.
///
/// Previously this update was silently dropped — users who received a refund
/// kept premium access until expiry. This handler revokes the subscription
/// immediately.
fn refunded_payment_handler(deps: HandlerDeps) -> UpdateHandler<HandlerError> {
    Update::filter_message()
        .filter(|msg: Message| matches!(msg.kind, teloxide::types::MessageKind::RefundedPayment(_)))
        .endpoint(move |bot: Bot, msg: Message| {
            let deps = deps.clone();
            async move {
                use crate::core::subscription;
                use crate::telegram::notifications::notify_admin_text;

                log::info!("Received refunded_payment message");

                // Extract the refunded_payment from MessageKind
                let refund = match &msg.kind {
                    teloxide::types::MessageKind::RefundedPayment(mrp) => mrp.refunded_payment.clone(),
                    _ => {
                        log::error!("refunded_payment_handler fired but MessageKind is not RefundedPayment");
                        return Ok(());
                    }
                };

                if let Err(e) =
                    subscription::handle_refunded_payment(&bot, &msg, &refund, Arc::clone(&deps.shared_storage)).await
                {
                    log::error!("Failed to handle refunded payment: {:?}", e);
                    notify_admin_text(
                        &bot,
                        &format!("REFUND HANDLER ERROR\nchat_id: {}\nerror: {:?}", msg.chat.id.0, e),
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

                if let Err(e) = handle_update_cookies_command(
                    deps.db_pool.clone(),
                    deps.shared_storage.clone(),
                    &bot,
                    msg.chat.id,
                    user_id,
                    message_text,
                )
                .await
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

/// Handler for /update_ig_cookies admin command (hidden, not in Command enum)
fn update_ig_cookies_handler(deps: HandlerDeps) -> UpdateHandler<HandlerError> {
    Update::filter_message()
        .filter(|msg: Message| {
            msg.text()
                .map(|text| text.starts_with("/update_ig_cookies"))
                .unwrap_or(false)
        })
        .endpoint(move |bot: Bot, msg: Message| {
            let deps = deps.clone();
            async move {
                use crate::telegram::handle_update_ig_cookies_command;

                log::info!("🎯 /update_ig_cookies handler matched");
                let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                let message_text = msg.text().unwrap_or_default();

                if let Err(e) = handle_update_ig_cookies_command(
                    deps.db_pool.clone(),
                    deps.shared_storage.clone(),
                    &bot,
                    msg.chat.id,
                    user_id,
                    message_text,
                )
                .await
                {
                    log::error!("❌ /update_ig_cookies handler failed for user {}: {}", user_id, e);
                    let _ = bot
                        .send_message(msg.chat.id, format!("❌ /update_ig_cookies failed: {}", e))
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

/// Handler for /send admin command (hidden, not in Command enum)
fn send_handler(deps: HandlerDeps) -> UpdateHandler<HandlerError> {
    Update::filter_message()
        .filter(|msg: Message| msg.text().map(|text| text.starts_with("/send ")).unwrap_or(false))
        .endpoint(move |bot: Bot, msg: Message| {
            let deps = deps.clone();
            async move {
                use crate::telegram::handle_send_command;

                let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                let message_text = msg.text().unwrap_or_default();

                if let Err(e) = handle_send_command(
                    &bot,
                    msg.chat.id,
                    user_id,
                    message_text,
                    deps.db_pool.clone(),
                    deps.shared_storage.clone(),
                )
                .await
                {
                    log::error!("/send handler failed for user {}: {}", user_id, e);
                    let _ = bot.send_message(msg.chat.id, format!("Error: {}", e)).await;
                }
                Ok(())
            }
        })
}

/// Handler for /broadcast admin command (hidden, not in Command enum)
fn broadcast_handler(deps: HandlerDeps) -> UpdateHandler<HandlerError> {
    Update::filter_message()
        .filter(|msg: Message| msg.text().map(|text| text.starts_with("/broadcast")).unwrap_or(false))
        .endpoint(move |bot: Bot, msg: Message| {
            let deps = deps.clone();
            async move {
                use crate::telegram::handle_broadcast_command;

                let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                let message_text = msg.text().unwrap_or_default();

                if let Err(e) = handle_broadcast_command(
                    &bot,
                    msg.chat.id,
                    user_id,
                    message_text,
                    deps.db_pool.clone(),
                    deps.shared_storage.clone(),
                )
                .await
                {
                    log::error!("/broadcast handler failed for user {}: {}", user_id, e);
                    let _ = bot.send_message(msg.chat.id, format!("Error: {}", e)).await;
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

    Update::filter_message()
        .filter(|msg: Message| is_fresh_message(&msg))
        .branch(
            dptree::entry()
                .filter_command::<Command>()
                .endpoint(move |bot: Bot, msg: Message, cmd: Command| {
                    let deps = deps.clone();
                    async move {
                        log::info!("🎯 Received command: {:?} from chat {}", cmd, msg.chat.id);
                        crate::core::metrics::record_command(&format!("{:?}", cmd).to_lowercase());

                        match cmd {
                            Command::Start => {
                                handle_start_command(&bot, &msg, &deps).await?;
                            }
                            Command::Settings => {
                                let _ = show_main_menu(
                                    &bot,
                                    msg.chat.id,
                                    deps.db_pool.clone(),
                                    deps.shared_storage.clone(),
                                )
                                .await;
                            }
                            Command::Info => {
                                log::info!("⚡ Command::Info matched, calling handle_info_command");
                                match handle_info_command(
                                    bot.clone(),
                                    msg.clone(),
                                    deps.db_pool.clone(),
                                    deps.shared_storage.clone(),
                                )
                                .await
                                {
                                    Ok(_) => log::info!("✅ handle_info_command completed successfully"),
                                    Err(e) => log::error!("❌ handle_info_command failed: {:?}", e),
                                }
                            }
                            Command::Downsub => {
                                let _ = handle_downsub_command(
                                    bot.clone(),
                                    msg.clone(),
                                    deps.db_pool.clone(),
                                    deps.shared_storage.clone(),
                                    deps.downsub_gateway.clone(),
                                    deps.subtitle_cache.clone(),
                                )
                                .await;
                            }
                            Command::History => {
                                let _ =
                                    show_history(&bot, msg.chat.id, deps.db_pool.clone(), deps.shared_storage.clone())
                                        .await;
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
                                match show_user_stats(
                                    &bot,
                                    msg.chat.id,
                                    deps.db_pool.clone(),
                                    deps.shared_storage.clone(),
                                )
                                .await
                                {
                                    Ok(_) => log::info!("Stats sent successfully"),
                                    Err(e) => log::error!("Failed to show user stats: {:?}", e),
                                }
                            }
                            Command::Export => {
                                let _ = show_export_menu(
                                    &bot,
                                    msg.chat.id,
                                    deps.db_pool.clone(),
                                    deps.shared_storage.clone(),
                                )
                                .await;
                            }
                            Command::Backup => {
                                let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                                let _ = handle_backup_command(&bot, msg.chat.id, user_id).await;
                            }
                            Command::Plan => {
                                let _ = show_subscription_info(
                                    &bot,
                                    msg.chat.id,
                                    deps.db_pool.clone(),
                                    deps.shared_storage.clone(),
                                )
                                .await;
                            }
                            Command::Users => {
                                let username = msg.from.as_ref().and_then(|u| u.username.as_deref());
                                let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                                let _ = handle_users_command(
                                    &bot,
                                    msg.chat.id,
                                    username,
                                    user_id,
                                    deps.db_pool.clone(),
                                    deps.shared_storage.clone(),
                                )
                                .await;
                            }
                            Command::Setplan => {
                                let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                                let message_text = msg.text().unwrap_or("");
                                let _ = handle_setplan_command(
                                    &bot,
                                    msg.chat.id,
                                    user_id,
                                    message_text,
                                    deps.shared_storage.clone(),
                                )
                                .await;
                            }
                            Command::Transactions => {
                                let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                                let _ = handle_transactions_command(&bot, msg.chat.id, user_id).await;
                            }
                            Command::Admin => {
                                let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                                if let Err(e) =
                                    handle_admin_command(&bot, msg.chat.id, user_id, deps.shared_storage.clone()).await
                                {
                                    log::error!("/admin command failed: {:#}", e);
                                    let _ = bot.send_message(msg.chat.id, format!("❌ Admin error: {}", e)).await;
                                }
                            }
                            Command::Charges => {
                                let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                                let message_text = msg.text().unwrap_or("");
                                let args = message_text.strip_prefix("/charges").unwrap_or("").trim();
                                let _ = handle_charges_command(
                                    &bot,
                                    msg.chat.id,
                                    user_id,
                                    deps.shared_storage.clone(),
                                    args,
                                )
                                .await;
                            }
                            Command::DownloadTg => {
                                let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                                let username = msg.from.as_ref().and_then(|u| u.username.as_deref());
                                let message_text = msg.text().unwrap_or("");
                                let _ = handle_download_tg_command(&bot, msg.chat.id, user_id, username, message_text)
                                    .await;
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
                                    deps.shared_storage.clone(),
                                    message_text,
                                )
                                .await;
                            }
                            Command::Analytics => {
                                let _ = handle_analytics_command(
                                    bot.clone(),
                                    msg.clone(),
                                    deps.db_pool.clone(),
                                    deps.shared_storage.clone(),
                                )
                                .await;
                            }
                            Command::Health => {
                                let _ = handle_health_command(
                                    bot.clone(),
                                    msg.clone(),
                                    deps.db_pool.clone(),
                                    deps.shared_storage.clone(),
                                )
                                .await;
                            }
                            Command::DownsubHealth => {
                                let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                                let _ = handle_downsub_health_command(
                                    &bot,
                                    msg.chat.id,
                                    user_id,
                                    deps.downsub_gateway.clone(),
                                )
                                .await;
                            }
                            Command::Metrics => {
                                let _ = handle_metrics_command(
                                    bot.clone(),
                                    msg.clone(),
                                    deps.db_pool.clone(),
                                    deps.shared_storage.clone(),
                                    None,
                                )
                                .await;
                            }
                            Command::Revenue => {
                                let _ = handle_revenue_command(
                                    bot.clone(),
                                    msg.clone(),
                                    deps.db_pool.clone(),
                                    deps.shared_storage.clone(),
                                )
                                .await;
                            }
                            Command::BotApiSpeed => {
                                let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                                let _ = handle_botapi_speed_command(&bot, msg.chat.id, user_id).await;
                            }
                            Command::Version => {
                                let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                                let _ = handle_version_command(&bot, msg.chat.id, user_id).await;
                            }
                            Command::Subscriptions => {
                                crate::telegram::subscriptions::handle_subscriptions_command(
                                    &bot,
                                    msg.chat.id,
                                    &deps.db_pool,
                                    &deps.shared_storage,
                                )
                                .await;
                            }
                            Command::Player => {
                                crate::telegram::menu::player::handle_player_command(
                                    &bot,
                                    msg.chat.id,
                                    &deps.db_pool,
                                    &deps.shared_storage,
                                )
                                .await;
                            }
                            Command::Playlists => {
                                crate::telegram::menu::playlist::handle_playlists_command(
                                    &bot,
                                    msg.chat.id,
                                    &deps.db_pool,
                                    &deps.shared_storage,
                                )
                                .await;
                            }
                            Command::PlaylistIntegrations => {
                                crate::telegram::menu::playlist_integrations::handle_playlist_integrations_command(
                                    &bot,
                                    msg.chat.id,
                                    &deps.db_pool,
                                    &deps.shared_storage,
                                )
                                .await;
                            }
                            Command::ProxyStats => {
                                let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                                let _ = crate::telegram::handle_proxy_stats_command(&bot, msg.chat.id, user_id).await;
                            }
                            Command::ProxyReset => {
                                let user_id = msg.from.as_ref().and_then(|u| i64::try_from(u.id.0).ok()).unwrap_or(0);
                                let _ = crate::telegram::handle_proxy_reset_command(&bot, msg.chat.id, user_id).await;
                            }
                        }
                        Ok(())
                    }
                }),
        )
}

/// Handler for voice messages — shows effects menu
fn voice_message_handler(deps: HandlerDeps) -> UpdateHandler<HandlerError> {
    use crate::telegram::voice_effects::handle_voice_message;

    Update::filter_message()
        .filter(|msg: Message| is_fresh_message(&msg))
        .filter(|msg: Message| msg.voice().is_some())
        .endpoint(move |bot: Bot, msg: Message| {
            let deps = deps.clone();
            async move {
                if let Err(e) = handle_voice_message(bot, msg, deps.db_pool, deps.shared_storage).await {
                    log::error!("Voice effects handler error: {:?}", e);
                }
                Ok(())
            }
        })
}

/// Handler for regular messages (URLs, text)
fn message_handler(deps: HandlerDeps) -> UpdateHandler<HandlerError> {
    use crate::telegram::{handle_message, is_message_addressed_to_bot};

    let bot_username = deps.bot_username.clone();
    let bot_id = deps.bot_id;

    Update::filter_message()
        .filter(|msg: Message| is_fresh_message(&msg))
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
                    deps.shared_storage.clone(),
                    deps.alert_manager.clone(),
                )
                .await;

                // Log request and manage user
                if let Some(text) = msg.text() {
                    match &user_info_result {
                        Ok(Some(user)) => {
                            if let Err(e) = deps.shared_storage.log_request(user.telegram_id(), text).await {
                                log::warn!("Failed to log request: {}", e);
                            }
                        }
                        Ok(None) | Err(_) => {
                            let chat_id = msg.chat.id.0;
                            match deps.shared_storage.get_user(chat_id).await {
                                Ok(Some(user)) => {
                                    if let Err(e) = deps.shared_storage.log_request(user.telegram_id(), text).await {
                                        log::warn!("Failed to log request: {}", e);
                                    }
                                }
                                Ok(None) => {
                                    let username = msg.from.as_ref().and_then(|u| u.username.clone());
                                    let lang_code = msg.from.as_ref().and_then(|u| u.language_code.as_deref());
                                    if let Err(e) = deps
                                        .shared_storage
                                        .create_user_with_language(chat_id, username.clone(), lang_code)
                                        .await
                                    {
                                        log::error!("Failed to create user: {}", e);
                                    } else {
                                        if let Err(e) = deps.shared_storage.log_request(chat_id, text).await {
                                            log::warn!("Failed to log request for new user: {}", e);
                                        }
                                        // Notify admins about new user
                                        use crate::telegram::notifications::notify_admin_new_user;
                                        let bot_notify = bot.clone();
                                        let first_name = msg.from.as_ref().map(|u| u.first_name.clone());
                                        let lang_code_owned = msg.from.as_ref().and_then(|u| u.language_code.clone());
                                        let first_message = text.to_string();
                                        tokio::spawn(async move {
                                            notify_admin_new_user(
                                                &bot_notify,
                                                chat_id,
                                                username.as_deref(),
                                                first_name.as_deref(),
                                                lang_code_owned.as_deref(),
                                                Some(&first_message),
                                            )
                                            .await;
                                        });
                                    }
                                }
                                Err(e) => {
                                    log::error!("Failed to get user from storage: {}", e);
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
                let lang = i18n::user_lang_from_storage(&deps.shared_storage, user_id as i64).await;
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

/// Handler for inline queries (Vlipsy video search in any chat)
fn inline_query_handler(_deps: HandlerDeps) -> UpdateHandler<HandlerError> {
    use crate::telegram::inline_query::handle_inline_query;

    Update::filter_inline_query().endpoint(move |bot: Bot, query: InlineQuery| async move {
        if let Err(e) = handle_inline_query(bot, query).await {
            log::error!("Inline query handler error: {:?}", e);
        }
        Ok(())
    })
}

/// Handler for callback queries (inline keyboard buttons)
fn callback_handler(deps: HandlerDeps) -> UpdateHandler<HandlerError> {
    use crate::telegram::handle_menu_callback;

    Update::filter_callback_query().endpoint(move |bot: Bot, q: CallbackQuery| {
        let deps = deps.clone();
        async move {
            let result: teloxide::RequestError = match handle_menu_callback(
                bot,
                q,
                deps.db_pool.clone(),
                deps.shared_storage.clone(),
                deps.download_queue.clone(),
                deps.rate_limiter.clone(),
                deps.extension_registry.clone(),
                deps.downsub_gateway.clone(),
                deps.subtitle_cache.clone(),
            )
            .await
            {
                Ok(()) => return Ok(()),
                Err(e) => e,
            };
            Err(Box::new(result) as HandlerError)
        }
    })
}
