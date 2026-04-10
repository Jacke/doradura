use super::{download_helpers::download_file_from_telegram, escape_markdown, is_admin, MAX_MESSAGE_LENGTH};
use crate::core::types::Plan;
use crate::storage::db::DbPool;
use crate::storage::SharedStorage;
use crate::telegram::Bot;
use crate::telegram::BotExt;
use anyhow::Result;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::ParseMode;

/// Handle /users command - show list of all users
pub async fn handle_users_command(
    bot: &Bot,
    chat_id: ChatId,
    username: Option<&str>,
    user_id: i64,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> Result<()> {
    log::debug!("Users command: username={:?}, is_admin={}", username, is_admin(user_id));

    if !is_admin(user_id) {
        log::warn!("User {:?} tried to access /users command without permission", username);
        bot.send_message(chat_id, "❌ You don't have permission to execute this command.")
            .await?;
        return Ok(());
    }

    let _ = db_pool;
    let users = shared_storage.get_all_users().await?;

    log::debug!("Found {} users in database", users.len());

    if users.is_empty() {
        bot.send_md(chat_id, "👥 *User List*\n\nNo users in the database yet\\.")
            .await?;
        return Ok(());
    }

    // Calculate statistics
    let free_count = users.iter().filter(|u| u.plan == Plan::Free).count();
    let premium_count = users.iter().filter(|u| u.plan == Plan::Premium).count();
    let vip_count = users.iter().filter(|u| u.plan == Plan::Vip).count();
    let with_subscription = users.iter().filter(|u| u.telegram_charge_id.is_some()).count();
    let recurring_count = users.iter().filter(|u| u.is_recurring).count();

    let total_users = escape_markdown(&users.len().to_string());
    let free_escaped = escape_markdown(&free_count.to_string());
    let premium_escaped = escape_markdown(&premium_count.to_string());
    let vip_escaped = escape_markdown(&vip_count.to_string());
    let subs_escaped = escape_markdown(&with_subscription.to_string());
    let recurring_escaped = escape_markdown(&recurring_count.to_string());

    let mut text = format!(
        "👥 *User List* \\(total\\: {}\\)\n\n\
        📊 Statistics:\n\
        • 🌟 Free: {}\n\
        • ⭐ Premium: {}\n\
        • 👑 VIP: {}\n\
        • 💫 Active subscriptions: {}\n\
        • 🔄 With auto-renewal: {}\n\n\
        ━━━━━━━━━━━━━━━━━━━━\n\n",
        total_users, free_escaped, premium_escaped, vip_escaped, subs_escaped, recurring_escaped
    );

    let mut users_added = 0;

    for (idx, user) in users.iter().enumerate() {
        let username_str = user
            .username
            .as_ref()
            .map(|u| {
                let escaped = escape_markdown(u);
                format!("@{}", escaped)
            })
            .unwrap_or_else(|| {
                let id_escaped = escape_markdown(&user.telegram_id.to_string());
                format!("ID\\: {}", id_escaped)
            });

        let plan_emoji = user.plan.emoji();

        // Show subscription status
        let subscription_status = if user.telegram_charge_id.is_some() {
            let recurring_icon = if user.is_recurring { "🔄" } else { "" };
            let expires_info = if let Some(ref expires_at) = user.subscription_expires_at {
                // Show only date without time for compactness
                let date_part = expires_at.split(' ').next().unwrap_or(expires_at);
                escape_markdown(date_part)
            } else {
                "unlimited".to_string()
            };
            format!(" 💫{} until {}", recurring_icon, expires_info)
        } else if user.subscription_expires_at.is_some() {
            // Subscription existed but expired
            " ⏰".to_string()
        } else {
            "".to_string()
        };

        let plan_escaped = escape_markdown(user.plan.as_str());
        let idx_escaped = escape_markdown(&(idx + 1).to_string());
        let user_line = format!(
            "{}\\. {} {} {}{}\n",
            idx_escaped, username_str, plan_emoji, plan_escaped, subscription_status
        );

        // Check if adding this line would exceed the limit
        if text.len() + user_line.len() > MAX_MESSAGE_LENGTH {
            let remaining = escape_markdown(&(users.len() - users_added).to_string());
            text.push_str(&format!("\n\\.\\.\\. and {} more users", remaining));
            break;
        }

        text.push_str(&user_line);
        users_added += 1;
    }

    log::debug!(
        "Sending users list with {} users (text length: {})",
        users_added,
        text.len()
    );

    match bot.send_message(chat_id, &text).parse_mode(ParseMode::MarkdownV2).await {
        Ok(_) => {
            log::debug!("Successfully sent users list");
        }
        Err(e) => {
            log::error!("Failed to send users list: {:?}", e);
            // Try sending without Markdown if there was a formatting error
            let text_plain = text.replace("\\", "").replace("*", "");
            bot.send_message(
                chat_id,
                format!(
                    "❌ Error sending the list. Trying without formatting:\n\n{}",
                    text_plain
                ),
            )
            .await?;
        }
    }

    Ok(())
}

/// Handle /setplan command - change user's subscription plan
pub async fn handle_setplan_command(
    bot: &Bot,
    chat_id: ChatId,
    user_id: i64,
    message_text: &str,
    shared_storage: Arc<SharedStorage>,
) -> Result<()> {
    if !is_admin(user_id) {
        bot.send_message(chat_id, "❌ You don't have permission to execute this command.")
            .await?;
        return Ok(());
    }

    // Parse command: /setplan <user_id> <plan> [days]
    let parts: Vec<&str> = message_text.split_whitespace().collect();
    if parts.len() < 3 {
        bot.send_message(
            chat_id,
            "❌ *Invalid command format*\n\n\
            *Usage:*\n\
            `/setplan <user_id> <plan> [days]`\n\n\
            *Parameters:*\n\
            • `user_id` \\- Telegram user ID\n\
            • `plan` \\- Plan: free, premium or vip\n\
            • `days` \\- \\(optional\\) Number of days the subscription is valid\n\n\
            *Examples:*\n\
            `/setplan 123456789 premium` \\- set unlimited premium\n\
            `/setplan 123456789 premium 30` \\- premium for 30 days\n\
            `/setplan 123456789 free` \\- reset to free plan",
        )
        .parse_mode(ParseMode::MarkdownV2)
        .await?;
        return Ok(());
    }

    let user_id = match parts[1].parse::<i64>() {
        Ok(id) => id,
        Err(_) => {
            bot.send_message(chat_id, "❌ Invalid user_id format. Use a numeric ID.")
                .await?;
            return Ok(());
        }
    };

    let plan = parts[2];
    if !["free", "premium", "vip"].contains(&plan) {
        bot.send_message(chat_id, "❌ Invalid plan. Use: free, premium or vip")
            .await?;
        return Ok(());
    }

    // Parse optional days parameter
    let days = if parts.len() >= 4 {
        match parts[3].parse::<i32>() {
            Ok(d) if d > 0 => Some(d),
            Ok(_) => {
                bot.send_message(chat_id, "❌ Number of days must be a positive integer")
                    .await?;
                return Ok(());
            }
            Err(_) => {
                bot.send_message(chat_id, "❌ Invalid number of days format. Use a number.")
                    .await?;
                return Ok(());
            }
        }
    } else {
        None
    };

    let expiry_days = if plan == "free" { None } else { days };
    shared_storage
        .update_user_plan_with_expiry(user_id, plan, expiry_days)
        .await?;

    let (plan_emoji, plan_name) = match plan {
        "premium" => ("⭐", "Premium"),
        "vip" => ("👑", "VIP"),
        _ => ("🌟", "Free"),
    };

    // Prepare expiry info for messages
    let expiry_info = if let Some(days_count) = days {
        let expiry_date = chrono::Utc::now() + chrono::Duration::days(days_count as i64);
        let formatted_date = expiry_date.format("%Y-%m-%d").to_string();
        format!("\n📅 Valid until: {}", formatted_date)
    } else if plan == "free" {
        String::new()
    } else {
        "\n♾️ Unlimited subscription".to_string()
    };

    let expiry_info_escaped = expiry_info.replace("-", "\\-");

    // Send message to admin
    bot.send_message(
        chat_id,
        format!(
            "✅ User {} plan changed to {} {}{}",
            user_id, plan_emoji, plan, expiry_info
        ),
    )
    .await?;

    // Send notification to the user whose plan was changed
    let user_chat_id = ChatId(user_id);
    bot.send_message(
        user_chat_id,
        format!(
            "💳 *Subscription Plan Change*\n\n\
            Your plan has been changed by an administrator\\.\n\n\
            *New plan:* {} {}{}\n\n\
            Changes take effect immediately\\! 🎉",
            plan_emoji, plan_name, expiry_info_escaped
        ),
    )
    .parse_mode(ParseMode::MarkdownV2)
    .await?;

    Ok(())
}

/// Handle /admin command - show admin control panel
pub async fn handle_admin_command(
    bot: &Bot,
    chat_id: ChatId,
    user_id: i64,
    shared_storage: Arc<SharedStorage>,
) -> Result<()> {
    if !is_admin(user_id) {
        bot.send_message(chat_id, "❌ You don't have permission to execute this command.")
            .await?;
        return Ok(());
    }

    crate::telegram::menu::admin_users::show_user_list(bot, chat_id, None, &shared_storage, 0, Default::default())
        .await?;
    Ok(())
}

/// Handle /charges command - view all payment charges
pub async fn handle_charges_command(
    bot: &Bot,
    chat_id: ChatId,
    user_id: i64,
    shared_storage: Arc<SharedStorage>,
    args: &str,
) -> Result<()> {
    if !is_admin(user_id) {
        bot.send_message(chat_id, "❌ You don't have permission to execute this command.")
            .await?;
        return Ok(());
    }

    let args_trimmed = args.trim();

    // Handle stats request
    if args_trimmed == "stats" {
        match shared_storage.get_charges_stats().await {
            Ok((total_charges, total_amount, premium_count, vip_count, recurring_count)) => {
                let text = format!(
                    "📊 *Payment Statistics*\n\n\
                    💰 Total payments: {}\n\
                    ⭐ Total amount: {} Stars\n\
                    🌟 Premium subscriptions: {}\n\
                    💎 VIP subscriptions: {}\n\
                    🔄 Recurring: {}",
                    total_charges, total_amount, premium_count, vip_count, recurring_count
                );
                bot.send_md(chat_id, text).await?;
            }
            Err(e) => {
                bot.send_message(chat_id, format!("❌ Error fetching statistics: {}", e))
                    .await?;
            }
        }
        return Ok(());
    }

    // Parse user_id if provided
    let (plan_filter, user_filter) = if args_trimmed == "premium" {
        (Some("premium"), None)
    } else if args_trimmed == "vip" {
        (Some("vip"), None)
    } else if let Ok(user_id) = args_trimmed.parse::<i64>() {
        (None, Some(user_id))
    } else if args_trimmed.is_empty() {
        (None, None)
    } else {
        bot.send_message(
            chat_id,
            "❌ Usage: /charges [stats|premium|vip|user_id]\n\n\
            Examples:\n\
            • /charges - all payments (last 20)\n\
            • /charges stats - statistics\n\
            • /charges premium - Premium only\n\
            • /charges vip - VIP only\n\
            • /charges 123456789 - user payments",
        )
        .await?;
        return Ok(());
    };

    // Get charges
    let charges = if let Some(user_id) = user_filter {
        shared_storage.get_user_charges(user_id).await
    } else {
        shared_storage.get_all_charges(plan_filter, Some(20), 0).await
    };

    match charges {
        Ok(charges) => {
            if charges.is_empty() {
                bot.send_message(chat_id, "📭 No payments found.").await?;
                return Ok(());
            }

            let mut text = String::new();
            text.push_str("💳 *Payments*\n\n");

            for (idx, charge) in charges.iter().enumerate() {
                let plan_emoji = if charge.plan == Plan::Premium { "⭐" } else { "💎" };
                let recurring_mark = if charge.is_recurring { " 🔄" } else { "" };
                let first_mark = if charge.is_first_recurring { " (first)" } else { "" };

                text.push_str(&format!(
                    "{}\\. {} *{}*{}{}\n\
                    • User ID: `{}`\n\
                    • Amount: {} {}\n\
                    • Charge ID: `{}`\n\
                    • Date: {}\n",
                    idx + 1,
                    plan_emoji,
                    escape_markdown(&charge.plan.as_str().to_uppercase()),
                    recurring_mark,
                    first_mark,
                    charge.user_id,
                    charge.total_amount,
                    escape_markdown(&charge.currency),
                    escape_markdown(&charge.telegram_charge_id),
                    escape_markdown(&charge.payment_date),
                ));

                if let Some(ref exp_date) = charge.subscription_expiration_date {
                    text.push_str(&format!("• Expires: {}\n", escape_markdown(exp_date)));
                }

                text.push('\n');

                // Split into multiple messages if too long
                if text.len() > 3500 {
                    bot.send_message(chat_id, text.clone())
                        .parse_mode(ParseMode::MarkdownV2)
                        .await?;
                    text.clear();
                    text.push_str("💳 *Payments \\(continued\\)*\n\n");
                }
            }

            if !text.trim().is_empty() {
                bot.send_md(chat_id, text).await?;
            }
        }
        Err(e) => {
            bot.send_message(chat_id, format!("❌ Error fetching payments: {}", e))
                .await?;
        }
    }

    Ok(())
}

/// Handles the /download_tg command (admin only)
pub async fn handle_download_tg_command(
    bot: &Bot,
    chat_id: ChatId,
    user_id: i64,
    username: Option<&str>,
    message_text: &str,
) -> Result<()> {
    // Check admin permissions
    if !is_admin(user_id) {
        bot.send_message(chat_id, "❌ This command is only available to administrators.")
            .await?;
        return Ok(());
    }

    // Parse file_id from command
    let parts: Vec<&str> = message_text.split_whitespace().collect();
    if parts.len() < 2 {
        bot.send_message(
            chat_id,
            "❌ Usage: /download_tg <file_id>\n\n\
            Example:\n\
            /download_tg BQACAgIAAxkBAAIBCGXxxx...\n\n\
            To get a file_id:\n\
            1. Send the bot a file\n\
            2. Use Telegram Bot API methods to get the file_id\n\
            3. Or use the /getfile command (if implemented)",
        )
        .await?;
        return Ok(());
    }

    let file_id = parts[1];
    log::info!(
        "📥 Admin {} requested download of file_id: {}",
        username.unwrap_or("unknown"),
        file_id
    );

    // Send "processing" message
    let processing_msg = bot
        .send_message(chat_id, "⏳ Downloading file from Telegram...")
        .await?;

    // Download the file
    match download_file_from_telegram(bot, file_id, None).await {
        Ok(path) => {
            // Get file metadata
            let metadata = tokio::fs::metadata(&path).await?;
            let size_mb = metadata.len() as f64 / (1024.0 * 1024.0);
            let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("unknown");

            let success_message = format!(
                "✅ *File downloaded successfully\\!*\n\n\
                📁 Path: `{}`\n\
                📄 Name: `{}`\n\
                📊 Size: {:.2} MB\n\
                🆔 File ID: `{}`",
                escape_markdown(&path.display().to_string()),
                escape_markdown(filename),
                size_mb,
                escape_markdown(file_id),
            );

            // Delete processing message
            let _ = bot.delete_message(chat_id, processing_msg.id).await;

            // Send success message
            bot.send_md(chat_id, success_message).await?;

            log::info!("✅ Successfully downloaded file_id {} to {:?}", file_id, path);
        }
        Err(e) => {
            log::error!("❌ Failed to download file_id {}: {}", file_id, e);

            // Delete processing message
            let _ = bot.delete_message(chat_id, processing_msg.id).await;

            // Send error message
            let error_message = format!(
                "❌ Error downloading file:\n\n{}\n\n\
                Possible reasons:\n\
                • Invalid file_id\n\
                • File was deleted from Telegram\n\
                • File is too old (>1 hour for non-documents)\n\
                • No access rights to the file",
                escape_markdown(&e.to_string())
            );

            bot.send_md(chat_id, error_message).await?;
        }
    }

    Ok(())
}

/// Handles the /sent_files command (admin only)
pub async fn handle_sent_files_command(
    bot: &Bot,
    chat_id: ChatId,
    user_id: i64,
    username: Option<&str>,
    db_pool: std::sync::Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    message_text: &str,
) -> Result<()> {
    // Check admin permissions
    if !is_admin(user_id) {
        bot.send_message(chat_id, "❌ This command is only available to administrators.")
            .await?;
        return Ok(());
    }

    // Parse limit from command arguments
    let parts: Vec<&str> = message_text.split_whitespace().collect();
    let limit = if parts.len() >= 2 {
        parts[1].parse::<i32>().ok()
    } else {
        Some(50)
    };

    log::info!(
        "📋 Admin {} requested sent files list (limit: {:?})",
        username.unwrap_or("unknown"),
        limit
    );

    let _ = db_pool;

    match shared_storage.get_sent_files(limit).await {
        Ok(files) => {
            if files.is_empty() {
                bot.send_md(
                    chat_id,
                    "📭 *No sent files*\n\n\
                    Files with file\\_id will appear here after successfully sending to users\\.",
                )
                .await?;
                return Ok(());
            }

            // Build response message
            let mut response = format!("📋 *Sent Files* \\({} items\\)\n\n", files.len());

            for (idx, file) in files.iter().enumerate() {
                let user_display = if let Some(ref uname) = file.username {
                    format!("@{}", escape_markdown(uname))
                } else {
                    format!("ID: {}", file.user_id)
                };

                // Truncate title if too long
                let title = if file.title.chars().count() > 40 {
                    format!("{}...", file.title.chars().take(37).collect::<String>())
                } else {
                    file.title.clone()
                };

                response.push_str(&format!(
                    "{}\\. *{}*\n\
                    👤 {}\n\
                    📄 Format: `{}`\n\
                    🆔 File ID:\n`{}`\n\
                    📅 {}\n\n",
                    idx + 1,
                    escape_markdown(&title),
                    user_display,
                    escape_markdown(&file.format),
                    escape_markdown(&file.file_id),
                    escape_markdown(&file.downloaded_at[..16]), // Show only date and time
                ));
            }

            response.push_str(
                "\n💡 *Usage:*\n\
                `/download_tg <file_id>` \\- download a file\n\n\
                For more files: `/sent_files <limit>`",
            );

            // Send response with MarkdownV2
            bot.send_md(chat_id, response).await?;

            log::info!(
                "✅ Sent files list delivered to admin {}",
                username.unwrap_or("unknown")
            );
        }
        Err(e) => {
            log::error!("❌ Failed to retrieve sent files: {}", e);
            bot.send_message(
                chat_id,
                format!("❌ Error fetching file list:\n\n{}", escape_markdown(&e.to_string())),
            )
            .parse_mode(ParseMode::MarkdownV2)
            .await?;
        }
    }

    Ok(())
}
