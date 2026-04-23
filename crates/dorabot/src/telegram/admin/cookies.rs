use super::{
    browser::cookie_manager_request, download_helpers::download_file_from_telegram, escape_markdown, is_admin,
};
use crate::download::cookies;
use crate::download::ytdlp;
use crate::storage::SharedStorage;
use crate::telegram::Bot;
use crate::telegram::BotExt;
use anyhow::Result;
use std::sync::LazyLock;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, MessageId};

/// Cooldown period for cookie refresh notifications (6 hours)
const COOKIE_NOTIFICATION_COOLDOWN: Duration = Duration::from_secs(6 * 60 * 60);

/// Timestamp of the last cookie refresh notification sent to admin
static LAST_COOKIE_NOTIFICATION: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(|| Mutex::new(None));

/// Handles the /diagnose_cookies command (admin only)
///
/// Shows detailed diagnostic information about the current cookies file
pub async fn handle_diagnose_cookies_command(bot: &Bot, chat_id: ChatId, user_id: i64) -> Result<()> {
    if !is_admin(user_id) {
        bot.send_message(chat_id, "❌ This command is for administrators only.")
            .await?;
        return Ok(());
    }

    let processing_msg = bot.send_message(chat_id, "⏳ Analysing cookies...").await?;

    // Get diagnostic
    let diagnostic = cookies::diagnose_cookies_file().await;
    let report = diagnostic.format_report();

    // Delete processing message
    bot.try_delete(chat_id, processing_msg.id).await;

    // Send report
    let message = format!("🍪 *YouTube Cookies Diagnostics*\n\n{}", escape_markdown(&report));

    bot.send_md(chat_id, message).await?;

    // If cookies look valid structurally, offer to test with yt-dlp
    if diagnostic.is_valid {
        let keyboard = InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb(
            "🧪 Test with yt-dlp",
            "admin:test_cookies",
        )]]);

        bot.send_md_kb(chat_id, "Do you want to test cookies with yt\\-dlp?", keyboard)
            .await?;
    }

    Ok(())
}

/// Handles the /update_cookies command (admin only)
///
/// Starts a session to receive a cookies file and updates the YTDL_COOKIES_FILE
pub async fn handle_update_cookies_command(
    _db_pool: Arc<crate::storage::db::DbPool>,
    shared_storage: Arc<SharedStorage>,
    bot: &Bot,
    chat_id: ChatId,
    user_id: i64,
    _message_text: &str,
) -> Result<()> {
    log::info!(
        "🔐 /update_cookies command received from user_id={}, chat_id={}",
        user_id,
        chat_id
    );

    // Check admin permissions
    if !is_admin(user_id) {
        log::warn!("❌ Non-admin user {} attempted to use /update_cookies", user_id);
        bot.send_message(chat_id, "❌ This command is only available to administrators.")
            .await?;
        return Ok(());
    }

    log::info!("✅ Admin authentication passed for user_id={}", user_id);

    // Create cookies upload session
    let session = crate::storage::db::CookiesUploadSession {
        id: uuid::Uuid::new_v4().to_string(),
        user_id,
        created_at: chrono::Utc::now(),
        expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
    };

    shared_storage.upsert_cookies_upload_session(&session).await?;

    log::info!("✅ Created cookies upload session for admin {}", user_id);

    bot.send_md(
        chat_id,
        "📤 *Send your cookies file*\n\n\
        Send a txt file with cookies in Netscape HTTP Cookie File format\\.\n\n\
        *How to get cookies:*\n\
        1\\. Install a cookies export extension\n\
        2\\. Export cookies for youtube\\.com\n\
        3\\. Send the file here\n\n\
        ⏱ Session expires in 10 minutes\\.",
    )
    .await?;

    log::info!("🏁 /update_cookies command handler finished for admin {}", user_id);
    Ok(())
}

/// Handles the /update_ytdlp command (admin only)
///
/// Triggers yt-dlp update and reports before/after version.
pub async fn handle_update_ytdlp_command(bot: &Bot, chat_id: ChatId, user_id: i64, _message_text: &str) -> Result<()> {
    log::info!(
        "🔧 /update_ytdlp command received from user_id={}, chat_id={}",
        user_id,
        chat_id
    );

    if !is_admin(user_id) {
        log::warn!("❌ Non-admin user {} attempted to use /update_ytdlp", user_id);
        bot.send_message(chat_id, "❌ This command is only available to administrators.")
            .await?;
        return Ok(());
    }

    let before = super::system::get_ytdlp_version()
        .await
        .unwrap_or_else(|| "unknown".to_string());
    let processing_msg = bot.send_message(chat_id, "⏳ Updating yt-dlp...").await?;

    match ytdlp::check_and_update_ytdlp().await {
        Ok(_) => {
            let after = super::system::get_ytdlp_version()
                .await
                .unwrap_or_else(|| "unknown".to_string());
            let status = if before == after {
                "yt-dlp is already up to date"
            } else {
                "yt-dlp updated"
            };
            let text = format!("✅ {}\nVersion before: {}\nVersion after: {}", status, before, after);
            bot.edit_message_text(chat_id, processing_msg.id, text).await?;
        }
        Err(e) => {
            let text = format!("❌ Failed to update yt-dlp: {}", e);
            bot.edit_message_text(chat_id, processing_msg.id, text).await?;
        }
    }

    Ok(())
}

/// Sends a notification to admin about cookies needing refresh
pub async fn notify_admin_cookies_refresh(bot: &Bot, admin_id: i64, reason: &str) -> Result<()> {
    // Check cooldown period - don't spam admin with repeated notifications
    {
        let mut last_notification = LAST_COOKIE_NOTIFICATION.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(last_time) = *last_notification {
            let elapsed = last_time.elapsed();
            if elapsed < COOKIE_NOTIFICATION_COOLDOWN {
                let remaining = COOKIE_NOTIFICATION_COOLDOWN - elapsed;
                log::info!(
                    "⏸️  Skipping cookie refresh notification (cooldown active, {:.1} hours remaining)",
                    remaining.as_secs_f64() / 3600.0
                );
                return Ok(());
            }
        }
        // Update timestamp before sending to prevent race conditions
        *last_notification = Some(Instant::now());
    }

    // Try to get detailed cookie info from cookie manager
    let cookie_detail = match cookie_manager_request("GET", "/api/status").await {
        Ok(data) => {
            let cookie_count = data.get("cookie_count").and_then(|v| v.as_u64()).unwrap_or(0);
            let required_found: Vec<String> = data
                .get("required_found")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let required_missing: Vec<String> = data
                .get("required_missing")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();

            let found_str = if required_found.is_empty() {
                "none".to_string()
            } else {
                required_found.join(", ")
            };
            let missing_str = if required_missing.is_empty() {
                "none".to_string()
            } else {
                required_missing.join(", ")
            };

            format!(
                "\n\n*Cookies status \\({} items\\):*\n\
                 ✅ Found: {}\n\
                 ❌ Missing: {}",
                cookie_count,
                escape_markdown(&found_str),
                escape_markdown(&missing_str)
            )
        }
        Err(_) => String::new(),
    };

    let message = format!(
        "🔴 *YouTube cookies update required*\n\n\
        Reason: _{}_\
        {}\n\n\
        To update:\n\
        • /browser\\_login — log in via browser \\(recommended\\)\n\
        • /update\\_cookies — upload cookies file manually\n\
        • /browser\\_status — check cookie manager status\n\n\
        Without valid cookies YouTube video downloads may not work\\.",
        escape_markdown(reason),
        cookie_detail
    );

    match bot.send_md(ChatId(admin_id), message).await {
        Ok(_) => {
            log::info!("✅ Sent cookies refresh notification to admin {}", admin_id);
            Ok(())
        }
        Err(e) => {
            log::error!(
                "❌ Failed to send cookies refresh notification to admin {}: {}",
                admin_id,
                e
            );
            Err(e.into())
        }
    }
}

/// Age-gate probe state transition for admin notifications.
#[derive(Debug, Clone, Copy)]
pub enum AgeGateTransition {
    /// Age-verified cookies stopped passing the age-gate probe (regular cookies still OK).
    Lost,
    /// Age-verified cookies recovered after a previous Lost transition.
    Recovered,
}

/// Sends a single notification to admin about an age-gate probe transition.
///
/// Caller is responsible for edge-triggering — this fn has no cooldown because
/// the 2-state machine in `spawn_cookies_checker` already dedupes by tracking
/// the previous `ProbeState` across ticks.
pub async fn notify_admin_age_gate_state(bot: &Bot, admin_id: i64, transition: AgeGateTransition) -> Result<()> {
    let message = match transition {
        AgeGateTransition::Lost => {
            "⚠️ *Age\\-verified cookies lost*\n\n\
             Regular YouTube cookies still work, but the age\\-verification probe \
             \\(Rammstein \"Sonne\"\\) now fails with _\"Sign in to confirm your age\"_\\.\n\n\
             Non\\-gated videos keep working\\. 18\\+ videos will fail until \
             cookies are re\\-exported from a browser session that has completed \
             YouTube's age\\-confirmation step\\.\n\n\
             To fix:\n\
             • Open YouTube in a browser signed into an age\\-verified account\n\
             • Confirm age on any 18\\+ video once\n\
             • Re\\-export cookies and send via /update\\_cookies"
        }
        AgeGateTransition::Recovered => {
            "✅ *Age\\-verified cookies recovered*\n\n\
             Age\\-gated probe passes again — 18\\+ videos should work\\."
        }
    };

    match bot.send_md(ChatId(admin_id), message.to_string()).await {
        Ok(_) => {
            log::info!("✅ Sent age-gate {:?} notification to admin {}", transition, admin_id);
            Ok(())
        }
        Err(e) => {
            log::error!("❌ Failed to send age-gate notification to admin {}: {}", admin_id, e);
            Err(e.into())
        }
    }
}

pub async fn handle_cookies_file_upload(
    _db_pool: Arc<crate::storage::db::DbPool>,
    shared_storage: Arc<SharedStorage>,
    bot: &Bot,
    chat_id: ChatId,
    user_id: i64,
    document: &teloxide::types::Document,
) -> Result<()> {
    log::info!(
        "📤 Cookies file received from user_id={}, chat_id={}, file_id={}",
        user_id,
        chat_id,
        document.file.id
    );

    // Check if there's an active cookies upload session
    let session = shared_storage.get_active_cookies_upload_session(user_id).await?;
    if session.is_none() {
        log::warn!("❌ No active cookies upload session for user {}", user_id);
        return Ok(()); // Silently ignore if no session
    }

    log::info!("✅ Active cookies upload session found for user {}", user_id);

    // Send processing message
    let processing_msg = bot.send_message(chat_id, "⏳ Processing cookies file...").await?;

    // Download file
    let _file = bot.get_file(document.file.id.clone()).await?;
    let file_path = std::path::PathBuf::from(format!("/tmp/cookies_upload_{}.txt", user_id));

    match download_file_from_telegram(bot, &document.file.id.0, Some(file_path.clone())).await {
        Ok(_) => {
            log::info!("✅ Cookies file downloaded to: {:?}", file_path);

            // Read file content
            match fs_err::tokio::read_to_string(&file_path).await {
                Ok(content) => {
                    log::info!("✅ Cookies file read successfully, {} bytes", content.len());

                    // Update cookies file
                    let diagnostic = cookies::diagnose_cookies_content(&content);
                    log::info!(
                        "🍪 Cookies diagnostic: {} total, {} youtube, valid={}",
                        diagnostic.total_cookies,
                        diagnostic.youtube_cookies,
                        diagnostic.is_valid
                    );

                    match cookies::update_cookies_from_content(&content).await {
                        Ok(path) => {
                            log::info!("✅ Cookies file successfully written to: {:?}", path);

                            // Delete temp file
                            let _ = fs_err::tokio::remove_file(&file_path).await;

                            // Delete session
                            if let Err(e) = shared_storage.delete_cookies_upload_session_by_user(user_id).await {
                                log::warn!("Failed to delete cookies upload session for user {}: {}", user_id, e);
                            }

                            // Delete processing message
                            bot.try_delete(chat_id, processing_msg.id).await;

                            // Build detailed diagnostic report
                            let diagnostic_report = diagnostic.format_report();

                            if diagnostic.is_valid {
                                // Cookies look good structurally, now test with yt-dlp
                                let test_msg = bot.send_message(chat_id, "⏳ Testing cookies with YouTube...").await?;

                                let validation_result = cookies::validate_cookies().await;
                                bot.try_delete(chat_id, test_msg.id).await;

                                match validation_result {
                                    Ok(()) => {
                                        let success_message = format!(
                                            "✅ *Cookies updated and verified successfully\\!*\n\n\
                                            📁 Path: `{}`\n\n\
                                            {}\n\n\
                                            ✓ YouTube download test passed successfully\\!\n\n\
                                            The bot now uses the new cookies for video downloads\\.",
                                            escape_markdown(&path.display().to_string()),
                                            escape_markdown(&diagnostic_report)
                                        );

                                        bot.send_md(chat_id, success_message).await?;

                                        log::info!("✅ Cookies update completed successfully for admin {}", user_id);
                                    }
                                    Err(reason) => {
                                        let warning_message = format!(
                                            "⚠️ *Cookies updated, but YouTube test failed*\n\n\
                                            📁 Path: `{}`\n\n\
                                            {}\n\n\
                                            *⚠️ yt\\-dlp error:* {}\n\n\
                                            *Possible reasons:*\n\
                                            • YouTube blocked the IP address \\(need a different proxy\\)\n\
                                            • Cookies were rotated after export\n\
                                            • Account requires confirmation \\(captcha/SMS\\)\n\n\
                                            Try:\n\
                                            1\\. Open YouTube in the browser\n\
                                            2\\. Watch any video to the end\n\
                                            3\\. Export cookies again",
                                            escape_markdown(&path.display().to_string()),
                                            escape_markdown(&diagnostic_report),
                                            escape_markdown(&reason.to_string())
                                        );

                                        bot.send_md(chat_id, warning_message).await?;

                                        log::warn!(
                                            "⚠️ Cookies update: file valid but yt-dlp test failed for admin {}",
                                            user_id
                                        );
                                    }
                                }
                            } else {
                                // Cookies have structural issues - report them without testing
                                let warning_message = format!(
                                    "⚠️ *Cookies updated, but issues were found*\n\n\
                                    📁 Path: `{}`\n\n\
                                    {}\n\n\
                                    *How to fix:*\n\
                                    1\\. Log in to YouTube in the browser\n\
                                    2\\. Make sure you use the correct export extension\n\
                                    3\\. Export cookies again \\(\"Get cookies\\.txt LOCALLY\"\\)",
                                    escape_markdown(&path.display().to_string()),
                                    escape_markdown(&diagnostic_report)
                                );

                                bot.send_md(chat_id, warning_message).await?;

                                log::warn!(
                                    "⚠️ Cookies update: structural issues found for admin {}: {:?}",
                                    user_id,
                                    diagnostic.issues
                                );
                            }
                        }
                        Err(e) => {
                            log::error!("❌ Failed to update cookies file: {}", e);
                            let _ = fs_err::tokio::remove_file(&file_path).await;
                            bot.try_delete(chat_id, processing_msg.id).await;
                            shared_storage.delete_cookies_upload_session_by_user(user_id).await?;

                            let error_message = format!(
                                "❌ *Error updating cookies:*\n\n{}\n\n\
                                Possible reasons:\n\
                                • Invalid cookies file format\n\
                                • YTDL\\_COOKIES\\_FILE variable is not set\n\
                                • File write permission issues",
                                escape_markdown(&e.to_string())
                            );

                            bot.send_md(chat_id, error_message).await?;
                        }
                    }
                }
                Err(e) => {
                    log::error!("❌ Failed to read cookies file: {}", e);
                    let _ = fs_err::tokio::remove_file(&file_path).await;
                    bot.try_delete(chat_id, processing_msg.id).await;
                    shared_storage.delete_cookies_upload_session_by_user(user_id).await?;

                    bot.send_md(
                        chat_id,
                        format!("❌ *File read error:*\n\n{}", escape_markdown(&e.to_string())),
                    )
                    .await?;
                }
            }
        }
        Err(e) => {
            log::error!("❌ Failed to download cookies file: {}", e);
            bot.try_delete(chat_id, processing_msg.id).await;
            shared_storage.delete_cookies_upload_session_by_user(user_id).await?;

            bot.send_md(
                chat_id,
                format!("❌ *File download error:*\n\n{}", escape_markdown(&e.to_string())),
            )
            .await?;
        }
    }

    Ok(())
}

// ==================== Instagram Cookies Commands ====================

/// Handles the /update_ig_cookies admin command
pub async fn handle_update_ig_cookies_command(
    _db_pool: Arc<crate::storage::db::DbPool>,
    shared_storage: Arc<SharedStorage>,
    bot: &Bot,
    chat_id: ChatId,
    user_id: i64,
    _message_text: &str,
) -> Result<()> {
    log::info!(
        "🔐 /update_ig_cookies command received from user_id={}, chat_id={}",
        user_id,
        chat_id
    );

    if !is_admin(user_id) {
        log::warn!("❌ Non-admin user {} attempted to use /update_ig_cookies", user_id);
        bot.send_message(chat_id, "❌ This command is only available to administrators.")
            .await?;
        return Ok(());
    }

    let session = crate::storage::db::CookiesUploadSession {
        id: uuid::Uuid::new_v4().to_string(),
        user_id,
        created_at: chrono::Utc::now(),
        expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
    };

    shared_storage.upsert_ig_cookies_upload_session(&session).await?;

    log::info!("✅ Created IG cookies upload session for admin {}", user_id);

    bot.send_md(
        chat_id,
        "📤 *Send your Instagram cookies file*\n\n\
        Send a txt file with cookies in Netscape HTTP Cookie File format\\.\n\n\
        *How to get cookies:*\n\
        1\\. Install a cookies export extension \\(Get cookies\\.txt LOCALLY\\)\n\
        2\\. Log in to Instagram in the browser\n\
        3\\. Export cookies for instagram\\.com\n\
        4\\. Send the file here\n\n\
        *Key cookies:* `sessionid`, `csrftoken`, `ds_user_id`\n\n\
        ⏱ Session expires in 10 minutes\\.",
    )
    .await?;

    Ok(())
}

/// Handles Instagram cookies file upload after /update_ig_cookies command
pub async fn handle_ig_cookies_file_upload(
    _db_pool: Arc<crate::storage::db::DbPool>,
    shared_storage: Arc<SharedStorage>,
    bot: &Bot,
    chat_id: ChatId,
    user_id: i64,
    document: &teloxide::types::Document,
) -> Result<()> {
    log::info!(
        "📤 IG Cookies file received from user_id={}, chat_id={}, file_id={}",
        user_id,
        chat_id,
        document.file.id
    );

    let session = shared_storage.get_active_ig_cookies_upload_session(user_id).await?;
    if session.is_none() {
        log::warn!("❌ No active IG cookies upload session for user {}", user_id);
        return Ok(());
    }

    let processing_msg = bot
        .send_message(chat_id, "⏳ Processing Instagram cookies file...")
        .await?;

    let _file = bot.get_file(document.file.id.clone()).await?;
    let file_path = std::path::PathBuf::from(format!("/tmp/ig_cookies_upload_{}.txt", user_id));

    match download_file_from_telegram(bot, &document.file.id.0, Some(file_path.clone())).await {
        Ok(_) => match fs_err::tokio::read_to_string(&file_path).await {
            Ok(content) => {
                let diagnostic = cookies::diagnose_ig_cookies_content(&content);
                log::info!(
                    "🍪 IG Cookies diagnostic: {} total, {} instagram, valid={}",
                    diagnostic.total_cookies,
                    diagnostic.youtube_cookies,
                    diagnostic.is_valid
                );

                match cookies::update_ig_cookies_from_content(&content).await {
                    Ok(path) => {
                        let _ = fs_err::tokio::remove_file(&file_path).await;
                        shared_storage.delete_ig_cookies_upload_session_by_user(user_id).await?;
                        bot.try_delete(chat_id, processing_msg.id).await;

                        let diagnostic_report = diagnostic.format_report();

                        if diagnostic.is_valid {
                            let test_msg = bot.send_message(chat_id, "⏳ Testing Instagram cookies...").await?;

                            let validation_result = cookies::validate_ig_cookies().await;
                            bot.try_delete(chat_id, test_msg.id).await;

                            match validation_result {
                                Ok(()) => {
                                    let success_message = format!(
                                        "✅ *Instagram cookies updated successfully\\!*\n\n\
                                            📁 Path: `{}`\n\n\
                                            {}\n\n\
                                            The bot now uses Instagram cookies to access private content\\.",
                                        escape_markdown(&path.display().to_string()),
                                        escape_markdown(&diagnostic_report)
                                    );

                                    bot.send_md(chat_id, success_message).await?;
                                }
                                Err(reason) => {
                                    let warning_message = format!(
                                        "⚠️ *Instagram cookies updated, but test failed*\n\n\
                                            📁 Path: `{}`\n\n\
                                            {}\n\n\
                                            *⚠️ Error:* {}\n\n\
                                            Cookies saved and will be used for GraphQL requests\\.",
                                        escape_markdown(&path.display().to_string()),
                                        escape_markdown(&diagnostic_report),
                                        escape_markdown(&reason.to_string())
                                    );

                                    bot.send_md(chat_id, warning_message).await?;
                                }
                            }
                        } else {
                            let warning_message = format!(
                                "⚠️ *Instagram cookies updated, but issues were found*\n\n\
                                    📁 Path: `{}`\n\n\
                                    {}\n\n\
                                    *How to fix:*\n\
                                    1\\. Log in to Instagram in the browser\n\
                                    2\\. Export cookies again",
                                escape_markdown(&path.display().to_string()),
                                escape_markdown(&diagnostic_report)
                            );

                            bot.send_md(chat_id, warning_message).await?;
                        }
                    }
                    Err(e) => {
                        log::error!("❌ Failed to update IG cookies file: {}", e);
                        let _ = fs_err::tokio::remove_file(&file_path).await;
                        bot.try_delete(chat_id, processing_msg.id).await;
                        shared_storage.delete_ig_cookies_upload_session_by_user(user_id).await?;

                        let error_message = format!(
                            "❌ *Error updating Instagram cookies:*\n\n{}\n\n\
                                Possible causes:\n\
                                • Invalid cookies file format\n\
                                • Missing INSTAGRAM\\_COOKIES\\_FILE variable",
                            escape_markdown(&e.to_string())
                        );

                        bot.send_md(chat_id, error_message).await?;
                    }
                }
            }
            Err(e) => {
                log::error!("❌ Failed to read IG cookies file: {}", e);
                let _ = fs_err::tokio::remove_file(&file_path).await;
                bot.try_delete(chat_id, processing_msg.id).await;
                shared_storage.delete_ig_cookies_upload_session_by_user(user_id).await?;

                bot.send_md(
                    chat_id,
                    format!("❌ *File read error:*\n\n{}", escape_markdown(&e.to_string())),
                )
                .await?;
            }
        },
        Err(e) => {
            log::error!("❌ Failed to download IG cookies file: {}", e);
            bot.try_delete(chat_id, processing_msg.id).await;
            shared_storage.delete_ig_cookies_upload_session_by_user(user_id).await?;

            bot.send_md(
                chat_id,
                format!("❌ *File download error:*\n\n{}", escape_markdown(&e.to_string())),
            )
            .await?;
        }
    }

    Ok(())
}

/// Handles the admin:test_cookies callback - tests cookies with yt-dlp
pub async fn handle_test_cookies_callback(bot: &Bot, chat_id: ChatId, message_id: MessageId) -> Result<()> {
    bot.edit_md(chat_id, message_id, "⏳ Testing cookies with yt\\-dlp\\.\\.\\.")
        .await?;

    let result = cookies::validate_cookies().await;

    let text = match result {
        Ok(()) => "✅ *Cookies are working\\!*\n\n\
            Download test passed successfully\\.\n\
            Cookies are valid and can be used for downloading\\."
            .to_string(),
        Err(reason) => {
            format!(
                "❌ *Cookies are not working*\n\n\
                *Error:* {}\n\n\
                *Possible reasons:*\n\
                • YouTube blocked the IP address\n\
                • Cookies expired or were rotated\n\
                • Account requires confirmation\n\n\
                Use /update\\_cookies to upload new ones\\.",
                escape_markdown(&reason.to_string())
            )
        }
    };

    bot.edit_md(chat_id, message_id, text).await?;

    Ok(())
}
