//! Admin functionality for the Telegram bot
//!
//! This module contains all admin-related commands and utilities:
//! - User management (/users, /setplan, /admin)
//! - Database backup operations
//! - Markdown escaping utilities

use crate::downsub::DownsubGateway;
use anyhow::Result;
use regex::Regex;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use teloxide::prelude::*;
use teloxide::types::{
    InlineKeyboardButton, InlineKeyboardMarkup, ParseMode, Seconds, TransactionPartner, TransactionPartnerUserKind,
};

use crate::core::config;
use crate::download::cookies;

use crate::core::config::admin::{ADMIN_IDS, ADMIN_USER_ID};
use crate::storage::backup::{create_backup, list_backups};
use crate::storage::db::{get_all_users, get_connection, update_user_plan, update_user_plan_with_expiry, DbPool};
use std::path::PathBuf;
use url::Url;

/// Maximum message length for Telegram (with margin)
const MAX_MESSAGE_LENGTH: usize = 4000;
const DEFAULT_BOT_API_LOG_PATH: &str = "bot-api-data/logs/telegram-bot-api.log";
const DEFAULT_BOT_API_LOG_TAIL_BYTES: u64 = 2 * 1024 * 1024;

fn truncate_message(text: &str) -> String {
    if text.len() <= MAX_MESSAGE_LENGTH {
        return text.to_string();
    }
    let mut trimmed = text.chars().take(MAX_MESSAGE_LENGTH - 20).collect::<String>();
    trimmed.push_str("\n... (truncated)");
    trimmed
}

#[derive(Default)]
struct QueryData {
    start_time: Option<f64>,
    size_bytes: Option<u64>,
    method: Option<String>,
    response_time: Option<f64>,
}

/// Check if user is admin
pub fn is_admin(user_id: i64) -> bool {
    if !ADMIN_IDS.is_empty() {
        return ADMIN_IDS.contains(&user_id);
    }
    if *ADMIN_USER_ID != 0 {
        return *ADMIN_USER_ID == user_id;
    }
    false
}

/// Escapes special characters for MarkdownV2 format
///
/// # Arguments
/// * `text` - Text to escape
///
/// # Returns
/// Escaped text safe for MarkdownV2 parsing
pub fn escape_markdown(text: &str) -> String {
    let mut result = String::with_capacity(text.len() * 2);

    for c in text.chars() {
        match c {
            '\\' => result.push_str("\\\\"),
            '_' => result.push_str("\\_"),
            '*' => result.push_str("\\*"),
            '[' => result.push_str("\\["),
            ']' => result.push_str("\\]"),
            '(' => result.push_str("\\("),
            ')' => result.push_str("\\)"),
            '~' => result.push_str("\\~"),
            '`' => result.push_str("\\`"),
            '>' => result.push_str("\\>"),
            '#' => result.push_str("\\#"),
            '+' => result.push_str("\\+"),
            '-' => result.push_str("\\-"),
            '=' => result.push_str("\\="),
            '|' => result.push_str("\\|"),
            '{' => result.push_str("\\{"),
            '}' => result.push_str("\\}"),
            '.' => result.push_str("\\."),
            '!' => result.push_str("\\!"),
            _ => result.push(c),
        }
    }

    result
}

fn indent_lines(text: &str, indent: &str) -> String {
    text.lines()
        .map(|line| format!("{}{}", indent, line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_subscription_period_for_log(period: &Seconds) -> String {
    let seconds = period.seconds();
    let days = seconds as f64 / 86_400.0;
    let months = days / 30.0;
    format!("{seconds} seconds (~{days:.2} days, ~{months:.2} months)")
}

fn read_log_tail(path: &PathBuf, max_bytes: u64) -> Result<String, std::io::Error> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    if len > max_bytes {
        file.seek(SeekFrom::End(-(max_bytes as i64)))?;
    } else {
        file.seek(SeekFrom::Start(0))?;
    }

    let mut buf = String::new();
    file.read_to_string(&mut buf)?;
    Ok(buf)
}

fn is_local_bot_api(bot_api_url: &str) -> bool {
    !bot_api_url.contains("api.telegram.org")
}

struct BotApiUploadStat {
    method: String,
    size_bytes: u64,
    duration_secs: f64,
    response_time: f64,
}

struct BotApiUploadPending {
    method: String,
    size_bytes: u64,
    start_time: f64,
}

/// Handle /botapi_speed command - show upload speed stats from local Bot API logs (admin only)
pub async fn handle_botapi_speed_command(bot: &Bot, chat_id: ChatId, user_id: i64) -> Result<()> {
    if !is_admin(user_id) {
        bot.send_message(chat_id, "❌ У тебя нет прав для выполнения этой команды.")
            .await?;
        return Ok(());
    }

    let bot_api_url = match std::env::var("BOT_API_URL") {
        Ok(url) => url,
        Err(_) => {
            bot.send_message(chat_id, "⚠️ BOT_API_URL не задан. Локальный Bot API не используется.")
                .await?;
            return Ok(());
        }
    };

    if !is_local_bot_api(&bot_api_url) {
        bot.send_message(
            chat_id,
            "⚠️ Используется официальный Bot API. Локальные логи недоступны.",
        )
        .await?;
        return Ok(());
    }

    let log_path = std::env::var("BOT_API_LOG_PATH").unwrap_or_else(|_| DEFAULT_BOT_API_LOG_PATH.to_string());
    let log_path = PathBuf::from(log_path);

    let tail_bytes = std::env::var("BOT_API_LOG_TAIL_BYTES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_BOT_API_LOG_TAIL_BYTES);

    let content = match read_log_tail(&log_path, tail_bytes) {
        Ok(data) => data,
        Err(e) => {
            bot.send_message(
                chat_id,
                format!("❌ Не удалось прочитать лог Bot API: {} ({})", log_path.display(), e),
            )
            .await?;
            return Ok(());
        }
    };

    let start_re = Regex::new(r"\[(\d+\.\d+)\].*Query (0x[0-9a-f]+): .*method:\s*([a-z_]+).*\[size:(\d+)\]")
        .map_err(|e| anyhow::anyhow!("Failed to compile start regex: {}", e))?;
    let response_re = Regex::new(r"\[(\d+\.\d+)\].*Query (0x[0-9a-f]+): \[method:([a-z_]+)\]")
        .map_err(|e| anyhow::anyhow!("Failed to compile response regex: {}", e))?;

    let mut queries: HashMap<String, QueryData> = HashMap::new();

    for line in content.lines() {
        if let Some(caps) = start_re.captures(line) {
            let time = caps.get(1).and_then(|v| v.as_str().parse::<f64>().ok());
            let query_id = caps.get(2).map(|v| v.as_str().to_string());
            let method = caps.get(3).map(|v| v.as_str().to_string());
            let size = caps.get(4).and_then(|v| v.as_str().parse::<u64>().ok());

            if let (Some(time), Some(query_id), Some(method), Some(size)) = (time, query_id, method, size) {
                let entry = queries.entry(query_id).or_default();
                entry.start_time = Some(time);
                entry.size_bytes = Some(size);
                entry.method = Some(method);
            }
        }

        if let Some(caps) = response_re.captures(line) {
            let time = caps.get(1).and_then(|v| v.as_str().parse::<f64>().ok());
            let query_id = caps.get(2).map(|v| v.as_str().to_string());

            if let (Some(time), Some(query_id)) = (time, query_id) {
                let entry = queries.entry(query_id).or_default();
                entry.response_time = Some(time);
            }
        }
    }

    let mut completed = Vec::new();
    let mut pending = Vec::new();
    for (_id, entry) in queries {
        match (entry.start_time, entry.size_bytes, entry.method, entry.response_time) {
            (Some(start_time), Some(size_bytes), Some(method), Some(response_time)) => {
                let duration = response_time - start_time;
                if duration > 0.0 {
                    completed.push(BotApiUploadStat {
                        method,
                        size_bytes,
                        duration_secs: duration,
                        response_time,
                    });
                }
            }
            (Some(start_time), Some(size_bytes), Some(method), None) => {
                pending.push(BotApiUploadPending {
                    method,
                    size_bytes,
                    start_time,
                });
            }
            _ => {}
        }
    }

    completed.sort_by(|a, b| {
        b.response_time
            .partial_cmp(&a.response_time)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    pending.sort_by(|a, b| {
        b.start_time
            .partial_cmp(&a.start_time)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut text = String::new();
    text.push_str("📡 *Bot API upload speed*");
    text.push_str(&format!("\nURL: `{}`", escape_markdown(&bot_api_url)));
    text.push_str(&format!(
        "\nЛог: `{}`\n",
        escape_markdown(&log_path.display().to_string())
    ));

    if completed.is_empty() && pending.is_empty() {
        text.push_str("\nНет записей send* в последнем логе.");
        bot.send_message(chat_id, text)
            .parse_mode(ParseMode::MarkdownV2)
            .await?;
        return Ok(());
    }

    if !completed.is_empty() {
        text.push_str("\n\n✅ *Последние завершённые:*");
        for stat in completed.iter().take(5) {
            let size_mb = stat.size_bytes as f64 / (1024.0 * 1024.0);
            let speed_mbs = size_mb / stat.duration_secs;
            text.push_str(&format!(
                "\n• {}: {:.1} MB за {:.1} c \\(~{:.2} MB/s\\)",
                escape_markdown(&stat.method),
                size_mb,
                stat.duration_secs,
                speed_mbs
            ));
        }
    }

    if !pending.is_empty() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        text.push_str("\n\n⏳ *В процессе:*");
        for stat in pending.iter().take(3) {
            let size_mb = stat.size_bytes as f64 / (1024.0 * 1024.0);
            let elapsed = (now - stat.start_time).max(0.0);
            text.push_str(&format!(
                "\n• {}: {:.1} MB, уже {:.0} c",
                escape_markdown(&stat.method),
                size_mb,
                elapsed
            ));
        }
    }

    bot.send_message(chat_id, text)
        .parse_mode(ParseMode::MarkdownV2)
        .await?;
    Ok(())
}

fn format_transaction_partner_for_log(partner: &TransactionPartner) -> String {
    match partner {
        TransactionPartner::User(user_partner) => {
            let user = &user_partner.user;
            let mut lines = Vec::new();
            lines.push("Type: User payment".to_string());
            lines.push(format!("User ID: {}", user.id.0));

            let name = match (&user.first_name, &user.last_name) {
                (first, Some(last)) => format!("{} {}", first, last),
                (first, None) => first.to_string(),
            };
            lines.push(format!("Name: {}", escape_markdown(&name)));

            if let Some(username) = &user.username {
                lines.push(format!("Username: @{}", escape_markdown(username)));
            }
            if let Some(lang) = &user.language_code {
                lines.push(format!("Language: {}", escape_markdown(lang)));
            }

            lines.push(format!("Is premium: {}", user.is_premium));
            lines.push(format!("Is bot: {}", user.is_bot));

            lines.push("Payment details:".to_string());
            match &user_partner.kind {
                TransactionPartnerUserKind::InvoicePayment(invoice) => {
                    lines.push("  Payment type: Invoice payment".to_string());
                    if let Some(payload) = &invoice.invoice_payload {
                        lines.push(format!("  Invoice payload: {}", escape_markdown(payload)));
                    }
                    if let Some(period) = &invoice.subscription_period {
                        lines.push(format!(
                            "  Subscription period: {}",
                            format_subscription_period_for_log(period)
                        ));
                    }
                    if let Some(affiliate) = &invoice.affiliate {
                        lines.push(format!("  Affiliate: {}", escape_markdown(&format!("{:?}", affiliate))));
                    }
                }
                TransactionPartnerUserKind::PaidMediaPayment(media) => {
                    lines.push("  Payment type: Paid media payment".to_string());
                    lines.push(format!("  Media: {}", escape_markdown(&format!("{:?}", media))));
                }
                TransactionPartnerUserKind::GiftPurchase(gift) => {
                    lines.push("  Payment type: Gift purchase".to_string());
                    lines.push(format!("  Gift: {}", escape_markdown(&format!("{:?}", gift))));
                }
                TransactionPartnerUserKind::PremiumPurchase(premium) => {
                    lines.push("  Payment type: Premium purchase".to_string());
                    lines.push(format!("  Premium: {}", escape_markdown(&format!("{:?}", premium))));
                }
                TransactionPartnerUserKind::BusinessAccountTransfer => {
                    lines.push("  Payment type: Business account transfer".to_string());
                }
            }

            lines.join("\n")
        }
        TransactionPartner::Fragment(fragment) => {
            format!(
                "Type: Fragment withdrawal\nDetails: {}",
                escape_markdown(&format!("{:?}", fragment))
            )
        }
        TransactionPartner::TelegramAds => "Type: Telegram Ads payment".to_string(),
        TransactionPartner::TelegramApi(_) => "Type: Telegram API service".to_string(),
        TransactionPartner::Chat(chat) => {
            format!(
                "Type: Chat transaction\nDetails: {}",
                escape_markdown(&format!("{:?}", chat))
            )
        }
        TransactionPartner::AffiliateProgram(program) => {
            format!(
                "Type: Affiliate program\nDetails: {}",
                escape_markdown(&format!("{:?}", program))
            )
        }
        TransactionPartner::Other => "Type: Other".to_string(),
    }
}

/// Handle /transactions command - list recent Telegram Stars transactions (admin only)
pub async fn handle_transactions_command(bot: &Bot, chat_id: ChatId, user_id: i64) -> Result<()> {
    if !is_admin(user_id) {
        bot.send_message(chat_id, "❌ У тебя нет прав для выполнения этой команды.")
            .await?;
        return Ok(());
    }

    bot.send_message(chat_id, "⏳ Получаю список транзакций...").await?;

    match bot.get_star_transactions().await {
        Ok(star_transactions) => {
            if star_transactions.transactions.is_empty() {
                bot.send_message(chat_id, "📭 Транзакции не найдены.").await?;
                return Ok(());
            }

            let mut text = String::new();
            text.push_str("💫 *Последние транзакции Stars*\n\n");

            for (idx, tx) in star_transactions.transactions.iter().take(20).enumerate() {
                let date = tx.date.format("%Y-%m-%d %H:%M:%S UTC");
                let amount = tx.amount;
                let id = tx.id.0.clone();

                text.push_str(&format!(
                    "{}\\. ID: `{}`\n• Дата: {}\n• Сумма: {}⭐\n",
                    idx + 1,
                    escape_markdown(&id),
                    escape_markdown(&date.to_string()),
                    amount
                ));

                if let Some(nanostar) = tx.nanostar_amount {
                    text.push_str(&format!("• Nanostar amount: {}\n", nanostar));
                }

                if let Some(source) = &tx.source {
                    let formatted = format_transaction_partner_for_log(source);
                    text.push_str("• Source:\n");
                    text.push_str(&indent_lines(&escape_markdown(&formatted), "  "));
                    text.push('\n');
                } else {
                    text.push_str("• Source: —\n");
                }
                if let Some(receiver) = &tx.receiver {
                    let formatted = format_transaction_partner_for_log(receiver);
                    text.push_str("• Receiver:\n");
                    text.push_str(&indent_lines(&escape_markdown(&formatted), "  "));
                    text.push('\n');
                } else {
                    text.push_str("• Receiver: —\n");
                }

                text.push('\n');

                if text.len() > 3500 {
                    text.push('…');
                    break;
                }
            }

            bot.send_message(chat_id, text)
                .parse_mode(ParseMode::MarkdownV2)
                .await?;
        }
        Err(e) => {
            log::error!("❌ Failed to fetch star transactions: {:?}", e);
            bot.send_message(chat_id, format!("❌ Не удалось получить транзакции: {:?}", e))
                .await?;
        }
    }

    Ok(())
}

/// Handle /backup command - create database backup
///
/// # Arguments
/// * `bot` - Bot instance
/// * `chat_id` - Chat ID where to send response
/// * `user_id` - Telegram user ID of the requester
pub async fn handle_backup_command(bot: &Bot, chat_id: ChatId, user_id: i64) -> Result<()> {
    if !is_admin(user_id) {
        bot.send_message(chat_id, "❌ У тебя нет прав для выполнения этой команды.")
            .await?;
        return Ok(());
    }

    match create_backup(&config::DATABASE_PATH) {
        Ok(backup_path) => {
            let backups = list_backups().unwrap_or_default();
            bot.send_message(
                chat_id,
                format!(
                    "✅ Бэкап создан успешно!\n\n📁 Путь: {}\n📊 Всего бэкапов: {}",
                    backup_path.display(),
                    backups.len()
                ),
            )
            .await?;
        }
        Err(e) => {
            bot.send_message(chat_id, format!("❌ Ошибка при создании бэкапа: {}", e))
                .await?;
        }
    }

    Ok(())
}

/// Handle /users command - show list of all users
///
/// # Arguments
/// * `bot` - Bot instance
/// * `chat_id` - Chat ID where to send response
/// * `username` - Username of the requester (for logs)
/// * `user_id` - Telegram user ID of the requester
/// * `db_pool` - Database connection pool
pub async fn handle_users_command(
    bot: &Bot,
    chat_id: ChatId,
    username: Option<&str>,
    user_id: i64,
    db_pool: Arc<DbPool>,
) -> Result<()> {
    log::debug!("Users command: username={:?}, is_admin={}", username, is_admin(user_id));

    if !is_admin(user_id) {
        log::warn!("User {:?} tried to access /users command without permission", username);
        bot.send_message(chat_id, "❌ У тебя нет прав для выполнения этой команды.")
            .await?;
        return Ok(());
    }

    let conn = get_connection(&db_pool)?;
    let users = get_all_users(&conn)?;

    log::debug!("Found {} users in database", users.len());

    if users.is_empty() {
        bot.send_message(
            chat_id,
            "👥 *Список пользователей*\n\nВ базе данных пока нет пользователей\\.",
        )
        .parse_mode(ParseMode::MarkdownV2)
        .await?;
        return Ok(());
    }

    // Calculate statistics
    let free_count = users.iter().filter(|u| u.plan == "free").count();
    let premium_count = users.iter().filter(|u| u.plan == "premium").count();
    let vip_count = users.iter().filter(|u| u.plan == "vip").count();
    let with_subscription = users.iter().filter(|u| u.telegram_charge_id.is_some()).count();
    let recurring_count = users.iter().filter(|u| u.is_recurring).count();

    let total_users = escape_markdown(&users.len().to_string());
    let free_escaped = escape_markdown(&free_count.to_string());
    let premium_escaped = escape_markdown(&premium_count.to_string());
    let vip_escaped = escape_markdown(&vip_count.to_string());
    let subs_escaped = escape_markdown(&with_subscription.to_string());
    let recurring_escaped = escape_markdown(&recurring_count.to_string());

    let mut text = format!(
        "👥 *Список пользователей* \\(всего\\: {}\\)\n\n\
        📊 Статистика:\n\
        • 🌟 Free: {}\n\
        • ⭐ Premium: {}\n\
        • 👑 VIP: {}\n\
        • 💫 Активных подписок: {}\n\
        • 🔄 С автопродлением: {}\n\n\
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

        let plan_emoji = match user.plan.as_str() {
            "premium" => "⭐",
            "vip" => "👑",
            _ => "🌟",
        };

        // Показываем статус подписки
        let subscription_status = if user.telegram_charge_id.is_some() {
            let recurring_icon = if user.is_recurring { "🔄" } else { "" };
            let expires_info = if let Some(ref expires_at) = user.subscription_expires_at {
                // Показываем только дату без времени для компактности
                let date_part = expires_at.split(' ').next().unwrap_or(expires_at);
                escape_markdown(date_part)
            } else {
                "бессрочно".to_string()
            };
            format!(" 💫{} до {}", recurring_icon, expires_info)
        } else if user.subscription_expires_at.is_some() {
            // Подписка была, но истекла
            " ⏰".to_string()
        } else {
            "".to_string()
        };

        let plan_escaped = escape_markdown(&user.plan);
        let idx_escaped = escape_markdown(&(idx + 1).to_string());
        let user_line = format!(
            "{}\\. {} {} {}{}\n",
            idx_escaped, username_str, plan_emoji, plan_escaped, subscription_status
        );

        // Check if adding this line would exceed the limit
        if text.len() + user_line.len() > MAX_MESSAGE_LENGTH {
            let remaining = escape_markdown(&(users.len() - users_added).to_string());
            text.push_str(&format!("\n\\.\\.\\. и еще {} пользователей", remaining));
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
                    "❌ Ошибка при отправке списка. Попробую без форматирования:\n\n{}",
                    text_plain
                ),
            )
            .await?;
        }
    }

    Ok(())
}

/// Handle /setplan command - change user's subscription plan
///
/// # Arguments
/// * `bot` - Bot instance
/// * `chat_id` - Chat ID where to send response
/// * `user_id` - Telegram user ID of the requester
/// * `message_text` - Full message text with command arguments
/// * `db_pool` - Database connection pool
pub async fn handle_setplan_command(
    bot: &Bot,
    chat_id: ChatId,
    user_id: i64,
    message_text: &str,
    db_pool: Arc<DbPool>,
) -> Result<()> {
    if !is_admin(user_id) {
        bot.send_message(chat_id, "❌ У тебя нет прав для выполнения этой команды.")
            .await?;
        return Ok(());
    }

    // Parse command: /setplan <user_id> <plan> [days]
    let parts: Vec<&str> = message_text.split_whitespace().collect();
    if parts.len() < 3 {
        bot.send_message(
            chat_id,
            "❌ *Неверный формат команды*\n\n\
            *Использование:*\n\
            `/setplan <user_id> <plan> [days]`\n\n\
            *Параметры:*\n\
            • `user_id` \\- Telegram ID пользователя\n\
            • `plan` \\- План: free, premium или vip\n\
            • `days` \\- \\(опционально\\) Количество дней действия подписки\n\n\
            *Примеры:*\n\
            `/setplan 123456789 premium` \\- установить бессрочный премиум\n\
            `/setplan 123456789 premium 30` \\- премиум на 30 дней\n\
            `/setplan 123456789 free` \\- сбросить на бесплатный план",
        )
        .parse_mode(ParseMode::MarkdownV2)
        .await?;
        return Ok(());
    }

    let user_id = match parts[1].parse::<i64>() {
        Ok(id) => id,
        Err(_) => {
            bot.send_message(chat_id, "❌ Неверный формат user_id. Используй числовой ID.")
                .await?;
            return Ok(());
        }
    };

    let plan = parts[2];
    if !["free", "premium", "vip"].contains(&plan) {
        bot.send_message(chat_id, "❌ Неверный план. Используй: free, premium или vip")
            .await?;
        return Ok(());
    }

    // Parse optional days parameter
    let days = if parts.len() >= 4 {
        match parts[3].parse::<i32>() {
            Ok(d) if d > 0 => Some(d),
            Ok(_) => {
                bot.send_message(chat_id, "❌ Количество дней должно быть положительным числом")
                    .await?;
                return Ok(());
            }
            Err(_) => {
                bot.send_message(chat_id, "❌ Неверный формат количества дней. Используй число.")
                    .await?;
                return Ok(());
            }
        }
    } else {
        None
    };

    let conn = get_connection(&db_pool)?;

    // Update plan with optional expiry date
    if let Some(days_count) = days {
        update_user_plan_with_expiry(&conn, user_id, plan, Some(days_count))?;
    } else {
        // For free plan, clear expiry; for paid plans without days, set as unlimited
        if plan == "free" {
            update_user_plan_with_expiry(&conn, user_id, plan, None)?;
        } else {
            update_user_plan(&conn, user_id, plan)?;
        }
    }

    let (plan_emoji, plan_name) = match plan {
        "premium" => ("⭐", "Premium"),
        "vip" => ("👑", "VIP"),
        _ => ("🌟", "Free"),
    };

    // Prepare expiry info for messages
    let expiry_info = if let Some(days_count) = days {
        let expiry_date = chrono::Utc::now() + chrono::Duration::days(days_count as i64);
        let formatted_date = expiry_date.format("%Y-%m-%d").to_string();
        format!("\n📅 Действует до: {}", formatted_date)
    } else if plan == "free" {
        String::new()
    } else {
        "\n♾️ Бессрочная подписка".to_string()
    };

    let expiry_info_escaped = expiry_info.replace("-", "\\-");

    // Send message to admin
    bot.send_message(
        chat_id,
        format!(
            "✅ План пользователя {} изменен на {} {}{}",
            user_id, plan_emoji, plan, expiry_info
        ),
    )
    .await?;

    // Send notification to the user whose plan was changed
    let user_chat_id = ChatId(user_id);
    bot.send_message(
        user_chat_id,
        format!(
            "💳 *Изменение плана подписки*\n\n\
            Твой план был изменен администратором\\.\n\n\
            *Новый план:* {} {}{}\n\n\
            Изменения вступят в силу немедленно\\! 🎉",
            plan_emoji, plan_name, expiry_info_escaped
        ),
    )
    .parse_mode(ParseMode::MarkdownV2)
    .await?;

    Ok(())
}

/// Handle /admin command - show admin control panel
///
/// # Arguments
/// * `bot` - Bot instance
/// * `chat_id` - Chat ID where to send response
/// * `user_id` - Telegram user ID of the requester
/// * `db_pool` - Database connection pool
pub async fn handle_admin_command(bot: &Bot, chat_id: ChatId, user_id: i64, db_pool: Arc<DbPool>) -> Result<()> {
    if !is_admin(user_id) {
        bot.send_message(chat_id, "❌ У тебя нет прав для выполнения этой команды.")
            .await?;
        return Ok(());
    }

    let conn = get_connection(&db_pool)?;
    let users = get_all_users(&conn)?;

    // Create inline keyboard with users (2 per row)
    let mut keyboard_rows = Vec::new();
    let mut current_row = Vec::new();

    for user in users.iter().take(20) {
        // Show first 20 users
        let username_display = user
            .username
            .as_ref()
            .map(|u| format!("@{}", u))
            .unwrap_or_else(|| format!("ID:{}", user.telegram_id));

        let plan_emoji = match user.plan.as_str() {
            "premium" => "⭐",
            "vip" => "👑",
            _ => "🌟",
        };

        let button_text = format!("{} {}", plan_emoji, username_display);
        let callback_data = format!("admin:user:{}", user.telegram_id);

        current_row.push(InlineKeyboardButton::callback(button_text, callback_data));

        // Every 2 buttons create a new row
        if current_row.len() == 2 {
            keyboard_rows.push(current_row.clone());
            current_row.clear();
        }
    }

    // Add remaining buttons if any
    if !current_row.is_empty() {
        keyboard_rows.push(current_row);
    }

    let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

    bot.send_message(
        chat_id,
        format!(
            "🔧 *Панель управления пользователями*\n\n\
            Выбери пользователя для управления:\n\n\
            Показано: {} из {}\n\n\
            💡 Для управления конкретным пользователем используй:\n\
            `/setplan <user_id> <plan>`",
            users.len().min(20),
            users.len()
        ),
    )
    .parse_mode(ParseMode::MarkdownV2)
    .reply_markup(keyboard)
    .await?;

    Ok(())
}

/// Handle /downsub_health command - check Downsub gRPC server health via gRPC
pub async fn handle_downsub_health_command(
    bot: &Bot,
    chat_id: ChatId,
    user_id: i64,
    downsub_gateway: Arc<DownsubGateway>,
) -> Result<()> {
    if !is_admin(user_id) {
        bot.send_message(chat_id, "❌ У тебя нет прав для выполнения этой команды.")
            .await?;
        return Ok(());
    }

    let response_text = match downsub_gateway.check_health().await {
        Ok(result) => {
            let mut text = format!(
                "✅ Downsub health ok\nstatus: {}\nversion: {}",
                result.status, result.version
            );
            if let Some(message) = result.message {
                text.push_str("\nmessage: ");
                text.push_str(&message);
            }
            if let Some(uptime) = result.uptime {
                text.push_str("\nuptime: ");
                text.push_str(&uptime);
            }
            text
        }
        Err(err) => format!("❌ Downsub health failed: {}", err),
    };

    bot.send_message(chat_id, truncate_message(&response_text)).await?;
    Ok(())
}

/// Handle /charges command - view all payment charges
///
/// # Arguments
/// * `bot` - Bot instance
/// * `chat_id` - Chat ID where to send response
/// * `user_id` - Telegram user ID of the requester
/// * `db_pool` - Database pool
/// * `args` - Optional arguments: "stats", "premium", "vip", or user_id
pub async fn handle_charges_command(
    bot: &Bot,
    chat_id: ChatId,
    user_id: i64,
    db_pool: std::sync::Arc<crate::storage::db::DbPool>,
    args: &str,
) -> Result<()> {
    if !is_admin(user_id) {
        bot.send_message(chat_id, "❌ У тебя нет прав для выполнения этой команды.")
            .await?;
        return Ok(());
    }

    let conn = match crate::storage::db::get_connection(&db_pool) {
        Ok(c) => c,
        Err(e) => {
            bot.send_message(chat_id, format!("❌ Ошибка подключения к БД: {}", e))
                .await?;
            return Ok(());
        }
    };

    let args_trimmed = args.trim();

    // Handle stats request
    if args_trimmed == "stats" {
        match crate::storage::db::get_charges_stats(&conn) {
            Ok((total_charges, total_amount, premium_count, vip_count, recurring_count)) => {
                let text = format!(
                    "📊 *Статистика платежей*\n\n\
                    💰 Всего платежей: {}\n\
                    ⭐ Общая сумма: {} Stars\n\
                    🌟 Premium подписок: {}\n\
                    💎 VIP подписок: {}\n\
                    🔄 Рекуррентных: {}",
                    total_charges, total_amount, premium_count, vip_count, recurring_count
                );
                bot.send_message(chat_id, text)
                    .parse_mode(ParseMode::MarkdownV2)
                    .await?;
            }
            Err(e) => {
                bot.send_message(chat_id, format!("❌ Ошибка получения статистики: {}", e))
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
            "❌ Использование: /charges [stats|premium|vip|user_id]\n\n\
            Примеры:\n\
            • /charges - все платежи (последние 20)\n\
            • /charges stats - статистика\n\
            • /charges premium - только Premium\n\
            • /charges vip - только VIP\n\
            • /charges 123456789 - платежи пользователя",
        )
        .await?;
        return Ok(());
    };

    // Get charges
    let charges = if let Some(user_id) = user_filter {
        crate::storage::db::get_user_charges(&conn, user_id)
    } else {
        crate::storage::db::get_all_charges(&conn, plan_filter, Some(20), 0)
    };

    match charges {
        Ok(charges) => {
            if charges.is_empty() {
                bot.send_message(chat_id, "📭 Платежи не найдены.").await?;
                return Ok(());
            }

            let mut text = String::new();
            text.push_str("💳 *Платежи*\n\n");

            for (idx, charge) in charges.iter().enumerate() {
                let plan_emoji = if charge.plan == "premium" { "⭐" } else { "💎" };
                let recurring_mark = if charge.is_recurring { " 🔄" } else { "" };
                let first_mark = if charge.is_first_recurring {
                    " (первый)"
                } else {
                    ""
                };

                text.push_str(&format!(
                    "{}\\. {} *{}*{}{}\n\
                    • User ID: `{}`\n\
                    • Сумма: {} {}\n\
                    • Charge ID: `{}`\n\
                    • Дата: {}\n",
                    idx + 1,
                    plan_emoji,
                    escape_markdown(&charge.plan.to_uppercase()),
                    recurring_mark,
                    first_mark,
                    charge.user_id,
                    charge.total_amount,
                    escape_markdown(&charge.currency),
                    escape_markdown(&charge.telegram_charge_id),
                    escape_markdown(&charge.payment_date),
                ));

                if let Some(ref exp_date) = charge.subscription_expiration_date {
                    text.push_str(&format!("• Истекает: {}\n", escape_markdown(exp_date)));
                }

                text.push('\n');

                // Split into multiple messages if too long
                if text.len() > 3500 {
                    bot.send_message(chat_id, text.clone())
                        .parse_mode(ParseMode::MarkdownV2)
                        .await?;
                    text.clear();
                    text.push_str("💳 *Платежи \\(продолжение\\)*\n\n");
                }
            }

            if !text.trim().is_empty() {
                bot.send_message(chat_id, text)
                    .parse_mode(ParseMode::MarkdownV2)
                    .await?;
            }
        }
        Err(e) => {
            bot.send_message(chat_id, format!("❌ Ошибка получения платежей: {}", e))
                .await?;
        }
    }

    Ok(())
}

/// Downloads a file from Telegram by file_id and saves it locally
///
/// # Arguments
/// * `bot` - Telegram bot instance
/// * `file_id` - Telegram file_id to download
/// * `destination_path` - Optional custom path to save the file. If None, saves to ./downloads/
///
/// # Returns
/// * `Ok(PathBuf)` - Path to the downloaded file
/// * `Err(anyhow::Error)` - If download fails
///
/// # Example
/// ```no_run
/// # use doradura::telegram::download_file_from_telegram;
/// # async fn run() -> anyhow::Result<()> {
/// let bot = teloxide::Bot::new("BOT_TOKEN");
/// let path = download_file_from_telegram(&bot, "BQACAgIAAxkBAAIBCGXxxx...", None).await?;
/// println!("File saved to: {:?}", path);
/// # Ok(())
/// # }
/// ```
pub async fn download_file_from_telegram(
    bot: &Bot,
    file_id: &str,
    destination_path: Option<PathBuf>,
) -> Result<PathBuf> {
    log::info!("📥 Starting download for file_id: {}", file_id);

    // Get file info from Telegram
    use teloxide::types::FileId;
    let file = bot.get_file(FileId(file_id.to_string())).await?;
    log::info!(
        "✅ File info retrieved: path = {}, size = {} bytes",
        file.path,
        file.size
    );

    // Determine destination path
    let dest_path = if let Some(custom_path) = destination_path {
        custom_path
    } else {
        // Create downloads directory if it doesn't exist
        let downloads_dir = PathBuf::from("./downloads");
        std::fs::create_dir_all(&downloads_dir)?;

        // Generate filename from file_id or use original filename from Telegram path
        // Telegram path format: "documents/file_123.pdf" or "photos/file_456.jpg"
        let filename = PathBuf::from(&file.path)
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("file_{}.bin", &file_id[..20.min(file_id.len())]));

        downloads_dir.join(filename)
    };

    log::info!("📂 Destination path: {:?}", dest_path);

    let (bot_api_url, bot_api_is_local) = std::env::var("BOT_API_URL")
        .ok()
        .map(|u| {
            let is_local = !u.contains("api.telegram.org");
            (Some(u), is_local)
        })
        .unwrap_or((None, false));

    let base_url_str = bot_api_url.as_deref().unwrap_or("https://api.telegram.org");

    // For local Bot API with BOT_API_DATA_DIR, copy file directly from mounted volume
    if bot_api_is_local {
        if let Ok(data_dir) = std::env::var("BOT_API_DATA_DIR") {
            // file.path is like: /var/lib/telegram-bot-api/8224275354:.../videos/file_1.mp4
            // Strip container prefix and use BOT_API_DATA_DIR instead
            let container_prefix = "/var/lib/telegram-bot-api/";
            if let Some(relative_path) = file.path.strip_prefix(container_prefix) {
                let source_path = std::path::Path::new(&data_dir).join(relative_path);
                log::info!("📂 Local Bot API: attempting direct file copy from {:?}", source_path);

                if source_path.exists() {
                    log::info!("✅ File exists locally, copying directly...");
                    tokio::fs::copy(&source_path, &dest_path).await?;
                    log::info!("✅ File copied successfully to: {:?}", dest_path);
                    log::info!(
                        "📊 File size: {} bytes ({:.2} MB)",
                        file.size,
                        file.size as f64 / (1024.0 * 1024.0)
                    );
                    return Ok(dest_path);
                } else {
                    log::warn!("⚠️ Local file not found at {:?}", source_path);
                }
            } else {
                log::warn!(
                    "⚠️ File path doesn't start with expected container prefix: {}",
                    file.path
                );
            }
        } else {
            log::warn!("⚠️ BOT_API_DATA_DIR not set, will try HTTP fallback (will likely fail)");
        }
    }

    let base_url =
        Url::parse(base_url_str).map_err(|e| anyhow::anyhow!("Invalid Bot API base URL for file download: {}", e))?;

    let file_url = build_file_url(&base_url, bot.token(), &file.path)?;

    if bot_api_is_local && !check_local_file_exists(base_url_str, bot.token(), &file.path).await? {
        return Err(anyhow::anyhow!(
            "File is not available on local Bot API server (base={}, path={})",
            base_url_str,
            file.path
        ));
    }

    // Download via HTTP (teloxide::Bot::download_file uses api.telegram.org internally)
    use tokio::io::AsyncWriteExt;
    let client = reqwest::Client::builder()
        .timeout(crate::config::network::timeout())
        .build()?;

    let tmp_path = dest_path.with_file_name(format!(
        "{}.part",
        dest_path.file_name().and_then(|n| n.to_str()).unwrap_or("download")
    ));

    let mut resp = client.get(file_url).send().await?;
    let status = resp.status();
    if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
        let body = resp.text().await.unwrap_or_default();
        tokio::fs::remove_file(&tmp_path).await.ok();
        return Err(anyhow::anyhow!(
            "Telegram file download failed (base={}, path={}, status={}): {}",
            base_url_str,
            file.path,
            status,
            body
        ));
    }

    let mut dst = tokio::fs::File::create(&tmp_path).await?;
    while let Some(chunk) = resp.chunk().await? {
        dst.write_all(&chunk).await?;
    }
    dst.flush().await.ok();
    tokio::fs::rename(&tmp_path, &dest_path).await?;

    log::info!("✅ File downloaded successfully to: {:?}", dest_path);
    log::info!(
        "📊 File size: {} bytes ({:.2} MB)",
        file.size,
        file.size as f64 / (1024.0 * 1024.0)
    );

    Ok(dest_path)
}

async fn check_local_file_exists(bot_api_url: &str, token: &str, file_path: &str) -> Result<bool> {
    let base =
        Url::parse(bot_api_url).map_err(|e| anyhow::anyhow!("Invalid BOT_API_URL for local file check: {}", e))?;
    let file_url = build_file_url(&base, token, file_path)?;

    let client = reqwest::Client::builder()
        .timeout(crate::config::network::timeout())
        .build()?;
    let resp = client
        .get(file_url)
        .header(reqwest::header::RANGE, "bytes=0-0")
        .send()
        .await?;

    let status = resp.status();
    log::info!(
        "🔎 Local Bot API file check: base={}, path={}, status={}",
        bot_api_url,
        file_path,
        status
    );

    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }
    if status.is_success() || status == reqwest::StatusCode::PARTIAL_CONTENT {
        return Ok(true);
    }

    let body = resp.text().await.unwrap_or_default();
    Err(anyhow::anyhow!(
        "Local Bot API file check failed (status={}): {}",
        status,
        body
    ))
}

fn build_file_url(base: &Url, token: &str, file_path: &str) -> Result<Url> {
    let mut url = base.clone();

    // For local Bot API, strip the container prefix
    let normalized_path = if !base.as_str().contains("api.telegram.org") {
        // Local Bot API: file_path is like "/var/lib/telegram-bot-api/8224275354:.../videos/file_1.mp4"
        // We need just the relative part: "8224275354:.../videos/file_1.mp4"
        let container_prefix = "/var/lib/telegram-bot-api/";
        file_path.strip_prefix(container_prefix).unwrap_or(file_path)
    } else {
        // Official API: use file_path as-is
        file_path
    };

    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| anyhow::anyhow!("BOT_API_URL cannot be a base URL"))?;
        segments.push("file");
        segments.push(&format!("bot{token}"));
        for seg in normalized_path.split('/') {
            if !seg.is_empty() {
                segments.push(seg);
            }
        }
    }
    Ok(url)
}

/// Handles the /download_tg command (admin only)
///
/// # Arguments
/// * `bot` - Telegram bot instance
/// * `chat_id` - Chat ID where the command was sent
/// * `user_id` - Telegram user ID of the requester
/// * `username` - Username of the requester (for logs)
/// * `message_text` - Full message text (e.g., "/download_tg BQACAgIAAxkBAAIBCGXxxx...")
///
/// # Behavior
/// - Checks if user is admin
/// - Parses file_id from command arguments
/// - Downloads file from Telegram
/// - Sends confirmation message with file info
///
/// # Example
/// User sends: `/download_tg BQACAgIAAxkBAAIBCGXxxx...`
/// Bot responds: `✅ Файл скачан: ./downloads/file_123.pdf (1.5 MB)`
pub async fn handle_download_tg_command(
    bot: &Bot,
    chat_id: ChatId,
    user_id: i64,
    username: Option<&str>,
    message_text: &str,
) -> Result<()> {
    // Check admin permissions
    if !is_admin(user_id) {
        bot.send_message(chat_id, "❌ Эта команда доступна только администраторам.")
            .await?;
        return Ok(());
    }

    // Parse file_id from command
    let parts: Vec<&str> = message_text.split_whitespace().collect();
    if parts.len() < 2 {
        bot.send_message(
            chat_id,
            "❌ Использование: /download_tg <file_id>\n\n\
            Пример:\n\
            /download_tg BQACAgIAAxkBAAIBCGXxxx...\n\n\
            Чтобы получить file_id:\n\
            1. Отправьте боту файл\n\
            2. Используйте методы Telegram Bot API для получения file_id\n\
            3. Или используйте команду /getfile (если реализовано)",
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
    let processing_msg = bot.send_message(chat_id, "⏳ Скачиваю файл из Telegram...").await?;

    // Download the file
    match download_file_from_telegram(bot, file_id, None).await {
        Ok(path) => {
            // Get file metadata
            let metadata = tokio::fs::metadata(&path).await?;
            let size_mb = metadata.len() as f64 / (1024.0 * 1024.0);
            let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("unknown");

            let success_message = format!(
                "✅ *Файл успешно скачан\\!*\n\n\
                📁 Путь: `{}`\n\
                📄 Имя: `{}`\n\
                📊 Размер: {:.2} MB\n\
                🆔 File ID: `{}`",
                escape_markdown(&path.display().to_string()),
                escape_markdown(filename),
                size_mb,
                escape_markdown(file_id),
            );

            // Delete processing message
            let _ = bot.delete_message(chat_id, processing_msg.id).await;

            // Send success message
            bot.send_message(chat_id, success_message)
                .parse_mode(ParseMode::MarkdownV2)
                .await?;

            log::info!("✅ Successfully downloaded file_id {} to {:?}", file_id, path);
        }
        Err(e) => {
            log::error!("❌ Failed to download file_id {}: {}", file_id, e);

            // Delete processing message
            let _ = bot.delete_message(chat_id, processing_msg.id).await;

            // Send error message
            let error_message = format!(
                "❌ Ошибка при скачивании файла:\n\n{}\n\n\
                Возможные причины:\n\
                • Неверный file_id\n\
                • Файл был удален из Telegram\n\
                • Файл слишком старый (>1 часа для не-документов)\n\
                • Нет прав доступа к файлу",
                escape_markdown(&e.to_string())
            );

            bot.send_message(chat_id, error_message)
                .parse_mode(ParseMode::MarkdownV2)
                .await?;
        }
    }

    Ok(())
}

/// Handles the /sent_files command (admin only)
///
/// # Arguments
/// * `bot` - Telegram bot instance
/// * `chat_id` - Chat ID where the command was sent
/// * `user_id` - Telegram user ID of the requester
/// * `username` - Username of the requester (for logs)
/// * `db_pool` - Database connection pool
/// * `message_text` - Full message text (e.g., "/sent_files" or "/sent_files 100")
///
/// # Behavior
/// - Checks if user is admin
/// - Retrieves files with file_id from database
/// - Displays paginated list of files with copy-able file_id
///
/// # Example
/// User sends: `/sent_files`
/// Bot responds with list of files and their file_id
pub async fn handle_sent_files_command(
    bot: &Bot,
    chat_id: ChatId,
    user_id: i64,
    username: Option<&str>,
    db_pool: std::sync::Arc<DbPool>,
    message_text: &str,
) -> Result<()> {
    use crate::storage::db::{get_connection, get_sent_files};

    // Check admin permissions
    if !is_admin(user_id) {
        bot.send_message(chat_id, "❌ Эта команда доступна только администраторам.")
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

    // Get connection from pool
    let conn = get_connection(&db_pool)?;

    // Retrieve sent files
    match get_sent_files(&conn, limit) {
        Ok(files) => {
            if files.is_empty() {
                bot.send_message(
                    chat_id,
                    "📭 *Нет отправленных файлов*\n\n\
                    Файлы с file\\_id появятся здесь после успешной отправки пользователям\\.",
                )
                .parse_mode(ParseMode::MarkdownV2)
                .await?;
                return Ok(());
            }

            // Build response message
            let mut response = format!("📋 *Отправленные файлы* \\({} шт\\.\\)\n\n", files.len());

            for (idx, file) in files.iter().enumerate() {
                let user_display = if let Some(ref uname) = file.username {
                    format!("@{}", escape_markdown(uname))
                } else {
                    format!("ID: {}", file.user_id)
                };

                // Truncate title if too long
                let title = if file.title.len() > 40 {
                    format!("{}...", &file.title[..37])
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
                "\n💡 *Использование:*\n\
                `/download_tg <file_id>` \\- скачать файл\n\n\
                Для большего количества файлов: `/sent_files <лимит>`",
            );

            // Send response with MarkdownV2
            bot.send_message(chat_id, response)
                .parse_mode(ParseMode::MarkdownV2)
                .await?;

            log::info!(
                "✅ Sent files list delivered to admin {}",
                username.unwrap_or("unknown")
            );
        }
        Err(e) => {
            log::error!("❌ Failed to retrieve sent files: {}", e);
            bot.send_message(
                chat_id,
                format!(
                    "❌ Ошибка при получении списка файлов:\n\n{}",
                    escape_markdown(&e.to_string())
                ),
            )
            .parse_mode(ParseMode::MarkdownV2)
            .await?;
        }
    }

    Ok(())
}

/// Handles the /update_cookies command (admin only)
///
/// Accepts a base64-encoded cookies file and updates the YTDL_COOKIES_FILE
///
/// # Arguments
/// * `bot` - Telegram bot instance
/// * `chat_id` - Chat ID where the command was sent
/// * `user_id` - Telegram user ID of the requester
/// * `message_text` - Full message text (e.g., "/update_cookies <base64_string>")
///
/// # Behavior
/// - Checks if user is admin
/// - Decodes and validates base64 cookies
/// - Updates the cookies file
/// - Validates new cookies work
/// - Sends confirmation message
///
/// # Example
/// User sends: `/update_cookies <base64_encoded_cookies>`
/// Bot responds: `✅ Cookies успешно обновлены и проверены!`
pub async fn handle_update_cookies_command(bot: &Bot, chat_id: ChatId, user_id: i64, message_text: &str) -> Result<()> {
    log::info!(
        "🔐 /update_cookies command received from user_id={}, chat_id={}",
        user_id,
        chat_id
    );

    // Check admin permissions
    if !is_admin(user_id) {
        log::warn!("❌ Non-admin user {} attempted to use /update_cookies", user_id);
        bot.send_message(chat_id, "❌ Эта команда доступна только администраторам.")
            .await?;
        return Ok(());
    }

    log::info!("✅ Admin authentication passed for user_id={}", user_id);

    // Parse base64 from command
    let parts: Vec<&str> = message_text.split_whitespace().collect();
    if parts.len() < 2 {
        log::warn!("⚠️  Admin {} called /update_cookies without base64 argument", user_id);
        bot.send_message(
            chat_id,
            "❌ *Использование:* `/update_cookies <base64>`\n\n\
            *Как получить base64:*\n\
            1\\. Экспортируй cookies из браузера \\(youtube\\.com\\)\n\
            2\\. Конвертируй в base64: `base64 youtube_cookies\\.txt`\n\
            3\\. Отправь результат этой командой\n\n\
            *Формат cookies:* Netscape HTTP Cookie File",
        )
        .parse_mode(ParseMode::MarkdownV2)
        .await?;
        return Ok(());
    }

    let cookies_b64 = parts[1..].join(" ");
    log::info!(
        "📥 Admin {} updating cookies (base64 length: {} bytes)",
        user_id,
        cookies_b64.len()
    );

    // Send "processing" message
    log::info!("⏳ Sending processing message to chat_id={}", chat_id);
    let processing_msg = bot.send_message(chat_id, "⏳ Обновляю cookies...").await?;

    // Update cookies file
    log::info!("🔄 Starting cookies file update...");
    match cookies::update_cookies_from_base64(&cookies_b64).await {
        Ok(path) => {
            log::info!("✅ Cookies file successfully written to: {:?}", path);

            // Validate new cookies
            log::info!("🔍 Starting cookies validation...");
            bot.edit_message_text(chat_id, processing_msg.id, "⏳ Проверяю новые cookies...")
                .await?;

            let validation_result = cookies::validate_cookies().await;
            if !validation_result {
                log::warn!("🔍 Validation failed after cookies update");
            }

            // Delete processing message
            let _ = bot.delete_message(chat_id, processing_msg.id).await;

            if validation_result {
                let success_message = format!(
                    "✅ *Cookies успешно обновлены и проверены\\!*\n\n\
                    📁 Путь: `{}`\n\
                    ✓ Cookies валидны и работают\n\n\
                    Бот теперь использует новые cookies для загрузки видео\\.",
                    escape_markdown(&path.display().to_string())
                );

                bot.send_message(chat_id, success_message)
                    .parse_mode(ParseMode::MarkdownV2)
                    .await?;

                log::info!("✅ /update_cookies completed successfully for admin {}", user_id);
            } else {
                let warning_message = format!(
                    "⚠️ *Cookies обновлены, но валидация не удалась*\n\n\
                    📁 Путь: `{}`\n\
                    ⚠️  Cookies могут быть невалидны\n\n\
                    Возможные причины:\n\
                    • Cookies устарели\n\
                    • Неверный формат файла\n\
                    • Сетевые проблемы\n\n\
                    Попробуй экспортировать cookies заново\\.",
                    escape_markdown(&path.display().to_string())
                );

                bot.send_message(chat_id, warning_message)
                    .parse_mode(ParseMode::MarkdownV2)
                    .await?;

                log::warn!(
                    "⚠️ /update_cookies completed with validation failure for admin {}",
                    user_id
                );
            }
        }
        Err(e) => {
            log::error!("❌ Failed to update cookies file: {}", e);
            log::error!("❌ Error details: {:?}", e);

            // Delete processing message
            let _ = bot.delete_message(chat_id, processing_msg.id).await;

            let error_message = format!(
                "❌ *Ошибка при обновлении cookies:*\n\n{}\n\n\
                Возможные причины:\n\
                • Неверный base64\n\
                • Неверный формат cookies\n\
                • Отсутствует переменная YTDL\\_COOKIES\\_FILE\n\
                • Проблемы с правами на запись файла",
                escape_markdown(&e.to_string())
            );

            bot.send_message(chat_id, error_message)
                .parse_mode(ParseMode::MarkdownV2)
                .await?;

            log::error!("❌ /update_cookies failed for admin {}", user_id);
        }
    }

    log::info!("🏁 /update_cookies command handler finished for admin {}", user_id);
    Ok(())
}

/// Sends a notification to admin about cookies needing refresh
///
/// # Arguments
/// * `bot` - Telegram bot instance
/// * `admin_id` - Admin's Telegram user ID
/// * `reason` - Reason why cookies need refresh (e.g., "validation failed", "file missing")
pub async fn notify_admin_cookies_refresh(bot: &Bot, admin_id: i64, reason: &str) -> Result<()> {
    let message = format!(
        "🔴 *Требуется обновление YouTube cookies*\n\n\
        Причина: {}\n\n\
        Для обновления:\n\
        1\\. Экспортируй cookies из браузера\n\
        2\\. Конвертируй в base64: `base64 youtube_cookies\\.txt`\n\
        3\\. Отправь командой: `/update_cookies <base64>`\n\n\
        Без валидных cookies загрузка видео с YouTube может не работать\\.",
        escape_markdown(reason)
    );

    match bot
        .send_message(ChatId(admin_id), message)
        .parse_mode(ParseMode::MarkdownV2)
        .await
    {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_markdown_basic() {
        assert_eq!(escape_markdown("hello"), "hello");
        assert_eq!(escape_markdown("hello_world"), "hello\\_world");
        assert_eq!(escape_markdown("hello*world"), "hello\\*world");
    }

    #[test]
    fn test_escape_markdown_complex() {
        let input = "Test: [link](url) *bold* _italic_ `code`";
        let expected = "Test: \\[link\\]\\(url\\) \\*bold\\* \\_italic\\_ \\`code\\`";
        assert_eq!(escape_markdown(input), expected);
    }

    #[test]
    fn test_escape_markdown_all_special_chars() {
        let input = r"\*[]()~`>#+-=|{}.!";
        let expected = r"\\\*\[\]\(\)\~\`\>\#\+\-\=\|\{\}\.\!";
        assert_eq!(escape_markdown(input), expected);
    }

    #[test]
    fn test_is_admin() {
        if !ADMIN_IDS.is_empty() {
            let admin_id = ADMIN_IDS[0];
            let non_admin_id = ADMIN_IDS.iter().max().copied().unwrap_or(0) + 1;
            assert!(is_admin(admin_id));
            assert!(!is_admin(non_admin_id));
        } else if *ADMIN_USER_ID != 0 {
            let admin_id = *ADMIN_USER_ID;
            assert!(is_admin(admin_id));
            assert!(!is_admin(admin_id + 1));
        } else {
            assert!(!is_admin(0));
        }
    }
}
