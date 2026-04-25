use super::{escape_markdown, indent_lines, is_admin, truncate_message};
use crate::core::config;
use crate::core::{BOT_API_RESPONSE_REGEX, BOT_API_START_SIMPLE_REGEX};
use crate::downsub::DownsubGateway;
use crate::storage::backup::{create_backup, list_backups};
use crate::telegram::Bot;
use crate::telegram::BotExt;
use anyhow::Result;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use teloxide::net::Download;
use teloxide::prelude::*;
use teloxide::types::{
    FileId, InlineKeyboardMarkup, MessageId, Seconds, TransactionPartner, TransactionPartnerUserKind,
};
use tokio::net::TcpStream;
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;
use url::Url;

use crate::download::ytdlp;

const DEFAULT_BOT_API_LOG_PATH: &str = "/data/logs/telegram-bot-api.log";
const DEFAULT_BOT_API_LOG_TAIL_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Default)]
struct QueryData {
    start_time: Option<f64>,
    size_bytes: Option<u64>,
    method: Option<String>,
    response_time: Option<f64>,
}

pub(super) async fn get_ytdlp_version() -> Option<String> {
    let ytdl_bin = &*config::YTDL_BIN;
    let output = TokioCommand::new(ytdl_bin).arg("--version").output().await.ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if version.is_empty() {
        None
    } else {
        Some(version)
    }
}

/// Check WARP SOCKS5 proxy connectivity
async fn check_warp_status() -> (bool, String, Option<String>) {
    let warp_proxy = match &*crate::core::config::proxy::WARP_PROXY {
        Some(url) => url.clone(),
        None => return (false, "Not configured".to_string(), None),
    };

    // Parse proxy URL to get host:port
    let url = match Url::parse(&warp_proxy) {
        Ok(u) => u,
        Err(_) => return (false, "Invalid URL".to_string(), Some(warp_proxy)),
    };

    let host = url.host_str().unwrap_or("127.0.0.1");
    let port = url.port().unwrap_or(1080);
    let addr = format!("{}:{}", host, port);

    match timeout(Duration::from_secs(3), TcpStream::connect(&addr)).await {
        Ok(Ok(_)) => (true, "Connected".to_string(), Some(warp_proxy)),
        Ok(Err(e)) => (false, format!("Error: {}", e), Some(warp_proxy)),
        Err(_) => (false, "Timeout".to_string(), Some(warp_proxy)),
    }
}

/// Check PO Token server (bgutil) on port 4416
async fn check_pot_server_status() -> (bool, String) {
    let client = match reqwest::Client::builder().timeout(Duration::from_secs(3)).build() {
        Ok(c) => c,
        Err(_) => return (false, "Client error".to_string()),
    };

    match client.get("http://127.0.0.1:4416").send().await {
        Ok(resp) => {
            // 404 is OK - server is running but no route at /
            if resp.status().is_success() || resp.status().as_u16() == 404 {
                (true, "Running".to_string())
            } else {
                (false, format!("HTTP {}", resp.status()))
            }
        }
        Err(e) => {
            if e.is_connect() {
                (false, "Not running".to_string())
            } else {
                (false, format!("{}", e))
            }
        }
    }
}

/// Check YouTube cookies file and required cookies presence.
///
/// YouTube 2025+ uses `__Secure-3PSID` as the primary auth cookie.
/// Legacy cookies (SID, HSID etc.) are optional if `__Secure-3PSID` is present.
async fn check_cookies_status() -> (bool, String, Vec<(&'static str, bool)>) {
    const KEY_COOKIES: &[&str] = &["__Secure-3PSID", "__Secure-3PAPISID", "LOGIN_INFO", "SID", "SAPISID"];

    let cookies_path = match crate::core::config::YTDL_COOKIES_FILE.as_ref() {
        Some(path) => path,
        None => return (false, "Path not configured".to_string(), vec![]),
    };

    let path = std::path::Path::new(cookies_path);
    if !path.exists() {
        return (false, "File not found".to_string(), vec![]);
    }

    let content = match fs_err::tokio::read_to_string(path).await {
        Ok(c) => c,
        Err(_) => return (false, "Read error".to_string(), vec![]),
    };

    let mut found_cookies = Vec::new();
    for &cookie_name in KEY_COOKIES {
        let found = content
            .lines()
            .any(|line| !line.starts_with('#') && line.contains(cookie_name));
        found_cookies.push((cookie_name, found));
    }

    // Valid if __Secure-3PSID is present (modern) OR SID is present (legacy)
    let has_modern = found_cookies.iter().any(|(n, f)| *n == "__Secure-3PSID" && *f);
    let has_legacy = found_cookies.iter().any(|(n, f)| *n == "SID" && *f);
    let is_valid = has_modern || has_legacy;
    let status = if is_valid { "Valid" } else { "Incomplete" };

    (is_valid, status.to_string(), found_cookies)
}

/// Handles the /version command (admin only)
///
/// Shows system diagnostics: yt-dlp version, WARP proxy status, PO Token server, and cookies.
pub async fn handle_version_command(bot: &Bot, chat_id: ChatId, user_id: i64) -> Result<()> {
    log::info!(
        "📦 /version command received from user_id={}, chat_id={}",
        user_id,
        chat_id
    );

    if !is_admin(user_id) {
        log::warn!("❌ Non-admin user {} attempted to use /version", user_id);
        bot.send_message(chat_id, "❌ This command is only available to administrators.")
            .await?;
        return Ok(());
    }

    // Collect all statuses in parallel
    let (ytdlp_version, warp_status, pot_status, cookies_status) = tokio::join!(
        get_ytdlp_version(),
        check_warp_status(),
        check_pot_server_status(),
        check_cookies_status()
    );

    let ytdlp_ver = ytdlp_version.unwrap_or_else(|| "unavailable".to_string());
    let ytdl_bin = &*config::YTDL_BIN;

    let (warp_ok, warp_msg, warp_url) = warp_status;

    // Mask proxy credentials: http://user:password@host:port → http://***:***@host:port
    let warp_display = warp_url.as_deref().map(|raw| {
        if let Ok(parsed) = Url::parse(raw) {
            let host = parsed.host_str().unwrap_or("?");
            let port = parsed.port().map(|p| format!(":{}", p)).unwrap_or_default();
            if !parsed.username().is_empty() {
                format!("{}://***:***@{}{}", parsed.scheme(), host, port)
            } else {
                format!("{}://{}{}", parsed.scheme(), host, port)
            }
        } else {
            "invalid URL".to_string()
        }
    });
    let (pot_ok, pot_msg) = pot_status;
    let (cookies_ok, cookies_msg, cookies_list) = cookies_status;

    // Format cookies list
    let cookies_detail = cookies_list
        .iter()
        .map(|(name, found)| format!("{} {}", name, if *found { "✓" } else { "✗" }))
        .collect::<Vec<_>>()
        .join("  ");

    let cookies_path = crate::core::config::YTDL_COOKIES_FILE
        .as_ref()
        .map(|s| s.as_str())
        .unwrap_or("not set");

    let text = format!(
        "📦 *Version and System Status*\n\n\
        🔧 *yt\\-dlp*\n\
        ├ Version: `{}`\n\
        └ Binary: `{}`\n\n\
        🌐 *WARP Proxy*\n\
        ├ Status: {} {}\n\
        └ Address: `{}`\n\n\
        🎫 *PO Token Server*\n\
        ├ Status: {} {}\n\
        └ Port: `4416`\n\n\
        🍪 *YouTube Cookies*\n\
        ├ Status: {} {}\n\
        ├ File: `{}`\n\
        └ {}",
        escape_markdown(&ytdlp_ver),
        escape_markdown(ytdl_bin),
        if warp_ok { "✅" } else { "❌" },
        escape_markdown(&warp_msg),
        escape_markdown(warp_display.as_deref().unwrap_or("not set")),
        if pot_ok { "✅" } else { "❌" },
        escape_markdown(&pot_msg),
        if cookies_ok { "✅" } else { "❌" },
        escape_markdown(&cookies_msg),
        escape_markdown(cookies_path),
        escape_markdown(&cookies_detail)
    );

    let keyboard = InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb(
        "🔄 Update yt-dlp".to_string(),
        "admin:update_ytdlp".to_string(),
    )]]);

    bot.send_md_kb(chat_id, text, keyboard).await?;

    Ok(())
}

/// Handles the callback for updating yt-dlp from /version command
pub async fn handle_update_ytdlp_callback(bot: &Bot, chat_id: ChatId, message_id: MessageId) -> Result<()> {
    let before = get_ytdlp_version().await.unwrap_or_else(|| "unknown".to_string());

    // Update message to show progress
    bot.edit_message_text(chat_id, message_id, "⏳ Updating yt-dlp...")
        .await?;

    match ytdlp::force_update_ytdlp().await {
        Ok(_) => {
            let after = get_ytdlp_version().await.unwrap_or_else(|| "unknown".to_string());
            let (status, emoji) = if before == after {
                ("yt\\-dlp is already up to date", "✅")
            } else {
                ("yt\\-dlp updated", "🎉")
            };
            let text = format!(
                "{} *{}*\n\n\
                Version before: `{}`\n\
                Version after: `{}`",
                emoji,
                status,
                escape_markdown(&before),
                escape_markdown(&after)
            );

            // Add button to check again
            let keyboard = InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb(
                "🔄 Check again".to_string(),
                "admin:check_ytdlp_version".to_string(),
            )]]);

            bot.edit_md_kb(chat_id, message_id, text, keyboard).await?;
        }
        Err(e) => {
            let text = format!(
                "❌ *Failed to update yt\\-dlp*\n\n\
                Error: `{}`",
                escape_markdown(&e.to_string())
            );
            bot.edit_md(chat_id, message_id, text).await?;
        }
    }

    Ok(())
}

/// Handles the callback for checking yt-dlp version
pub async fn handle_check_ytdlp_version_callback(bot: &Bot, chat_id: ChatId, message_id: MessageId) -> Result<()> {
    let version = get_ytdlp_version()
        .await
        .unwrap_or_else(|| "failed to retrieve".to_string());

    let ytdl_bin = &*config::YTDL_BIN;

    let text = format!(
        "📦 *yt\\-dlp version*\n\n\
        Version: `{}`\n\
        Binary: `{}`",
        escape_markdown(&version),
        escape_markdown(ytdl_bin)
    );

    let keyboard = InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb(
        "🔄 Update yt-dlp".to_string(),
        "admin:update_ytdlp".to_string(),
    )]]);

    bot.edit_md_kb(chat_id, message_id, text, keyboard).await?;

    Ok(())
}

pub(super) fn format_subscription_period_for_log(period: &Seconds) -> String {
    let seconds = period.seconds();
    let days = seconds as f64 / 86_400.0;
    let months = days / 30.0;
    format!("{seconds} seconds (~{days:.2} days, ~{months:.2} months)")
}

pub(super) fn read_log_tail(path: &PathBuf, max_bytes: u64) -> Result<String, std::io::Error> {
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
        bot.send_message(chat_id, "❌ You don't have permission to execute this command.")
            .await?;
        return Ok(());
    }

    let bot_api_url = match std::env::var("BOT_API_URL") {
        Ok(url) => url,
        Err(_) => {
            bot.send_message(chat_id, "⚠️ BOT_API_URL is not set. Local Bot API is not in use.")
                .await?;
            return Ok(());
        }
    };

    if !config::bot_api::is_local_url(&bot_api_url) {
        bot.send_message(chat_id, "⚠️ Using the official Bot API. Local logs are not available.")
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
                format!("❌ Failed to read Bot API log: {} ({})", log_path.display(), e),
            )
            .await?;
            return Ok(());
        }
    };

    // Use pre-compiled lazy regexes from crate::core
    let start_re = &*BOT_API_START_SIMPLE_REGEX;
    let response_re = &*BOT_API_RESPONSE_REGEX;

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
        "\nLog: `{}`\n",
        escape_markdown(&log_path.display().to_string())
    ));

    if completed.is_empty() && pending.is_empty() {
        text.push_str("\nNo send* entries found in the latest log.");
        bot.send_md(chat_id, text).await?;
        return Ok(());
    }

    if !completed.is_empty() {
        text.push_str("\n\n✅ *Latest completed:*");
        for stat in completed.iter().take(5) {
            let size_mb = stat.size_bytes as f64 / (1024.0 * 1024.0);
            let speed_mbs = size_mb / stat.duration_secs;
            text.push_str(&format!(
                "\n• {}: {:.1} MB in {:.1} s \\(~{:.2} MB/s\\)",
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
        text.push_str("\n\n⏳ *In progress:*");
        for stat in pending.iter().take(3) {
            let size_mb = stat.size_bytes as f64 / (1024.0 * 1024.0);
            let elapsed = (now - stat.start_time).max(0.0);
            text.push_str(&format!(
                "\n• {}: {:.1} MB, elapsed {:.0} s",
                escape_markdown(&stat.method),
                size_mb,
                elapsed
            ));
        }
    }

    bot.send_md(chat_id, text).await?;
    Ok(())
}

pub(super) fn format_transaction_partner_for_log(partner: &TransactionPartner) -> String {
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
        bot.send_message(chat_id, "❌ You don't have permission to execute this command.")
            .await?;
        return Ok(());
    }

    bot.send_message(chat_id, "⏳ Fetching transactions list...").await?;

    match bot.get_star_transactions().await {
        Ok(star_transactions) => {
            if star_transactions.transactions.is_empty() {
                bot.send_message(chat_id, "📭 No transactions found.").await?;
                return Ok(());
            }

            let mut text = String::new();
            text.push_str("💫 *Latest Stars Transactions*\n\n");

            for (idx, tx) in star_transactions.transactions.iter().take(20).enumerate() {
                let date = tx.date.format("%Y-%m-%d %H:%M:%S UTC");
                let amount = tx.amount;
                let id = tx.id.0.clone();

                text.push_str(&format!(
                    "{}\\. ID: `{}`\n• Date: {}\n• Amount: {}⭐\n",
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

            bot.send_md(chat_id, text).await?;
        }
        Err(e) => {
            log::error!("❌ Failed to fetch star transactions: {:?}", e);
            bot.send_message(chat_id, format!("❌ Failed to fetch transactions: {:?}", e))
                .await?;
        }
    }

    Ok(())
}

/// Handle /backup command - create database backup
pub async fn handle_backup_command(bot: &Bot, chat_id: ChatId, user_id: i64) -> Result<()> {
    if !is_admin(user_id) {
        bot.send_message(chat_id, "❌ You don't have permission to execute this command.")
            .await?;
        return Ok(());
    }

    match create_backup(&config::DATABASE_PATH) {
        Ok(backup_path) => {
            let backups = list_backups().unwrap_or_default();
            bot.send_message(
                chat_id,
                format!(
                    "✅ Backup created successfully!\n\n📁 Path: {}\n📊 Total backups: {}",
                    backup_path.display(),
                    backups.len()
                ),
            )
            .await?;
        }
        Err(e) => {
            bot.send_message(chat_id, format!("❌ Error creating backup: {}", e))
                .await?;
        }
    }

    Ok(())
}

/// Handle /downsub_health command - check Downsub gRPC server health via gRPC
/// `/test_circle_save <slot>` — stages an mp4 file (sent by the admin as a
/// Telegram document, *replied to* with this command) into
/// `${TEST_CIRCLES_DIR}/test_<slot>.mp4` for the `/test_circle` empirical
/// test. Valid slot values: `small`, `medium`, `max`.
///
/// Why a reply to a document and not a video — Telegram recompresses
/// uploads sent as `sendVideo`, which would defeat the point of staging
/// pre-encoded test files. Documents pass through untouched.
pub async fn handle_test_circle_save_command(bot: &Bot, msg: &Message, user_id: i64) -> Result<()> {
    if !is_admin(user_id) {
        bot.send_message(msg.chat.id, "❌ You don't have permission to execute this command.")
            .await?;
        return Ok(());
    }

    let text = msg.text().unwrap_or_default();
    let slot = text.split_whitespace().nth(1).unwrap_or("").to_lowercase();
    if !matches!(slot.as_str(), "small" | "medium" | "max") {
        bot.send_message(
            msg.chat.id,
            "Usage: reply to an mp4 file with `/test_circle_save <small|medium|max>`",
        )
        .await?;
        return Ok(());
    }

    let Some(reply) = msg.reply_to_message() else {
        bot.send_message(
            msg.chat.id,
            "❌ This command must be sent as a *reply* to an mp4 document.",
        )
        .await?;
        return Ok(());
    };

    let (file_id, file_size) = if let Some(doc) = reply.document() {
        (doc.file.id.0.clone(), doc.file.size)
    } else if let Some(vid) = reply.video() {
        (vid.file.id.0.clone(), vid.file.size)
    } else {
        bot.send_message(
            msg.chat.id,
            "❌ Reply target has no document or video. Send the mp4 as *file* and reply to it.",
        )
        .await?;
        return Ok(());
    };

    let dir = std::env::var("TEST_CIRCLES_DIR").unwrap_or_else(|_| "./test_circles".to_string());
    let dir_path = std::path::PathBuf::from(&dir);
    if let Err(e) = fs_err::tokio::create_dir_all(&dir_path).await {
        bot.send_message(msg.chat.id, format!("❌ Failed to create {dir}: {e}"))
            .await?;
        return Ok(());
    }
    let dest = dir_path.join(format!("test_{slot}.mp4"));

    bot.send_message(
        msg.chat.id,
        format!("⬇️ Downloading {file_size} bytes from Telegram → `{}`…", dest.display()),
    )
    .await?;

    // Resolve file_id → file_path via Bot API getFile, then stream to disk.
    let file_meta = match bot.get_file(FileId(file_id)).await {
        Ok(f) => f,
        Err(e) => {
            bot.send_message(msg.chat.id, format!("❌ getFile failed: {e}")).await?;
            return Ok(());
        }
    };
    let mut out = match fs_err::tokio::File::create(&dest).await {
        Ok(f) => f,
        Err(e) => {
            bot.send_message(msg.chat.id, format!("❌ create file failed: {e}"))
                .await?;
            return Ok(());
        }
    };
    if let Err(e) = bot.download_file(&file_meta.path, &mut out).await {
        bot.send_message(msg.chat.id, format!("❌ download failed: {e}"))
            .await?;
        return Ok(());
    }
    let actual_size = fs_err::tokio::metadata(&dest).await.map(|m| m.len()).unwrap_or(0);
    bot.send_message(
        msg.chat.id,
        format!(
            "✅ Saved {} ({:.1} MB) to `{}`. Run `/test_circle` once all three slots are filled.",
            slot,
            actual_size as f64 / 1024.0 / 1024.0,
            dest.display()
        ),
    )
    .await?;
    Ok(())
}

/// `/test_circle` — empirical-test command for tuning the video-note encoding
/// pipeline against Telegram's server-side re-encode.
///
/// Reads three pre-baked test files from `${TEST_CIRCLES_DIR}` (default
/// `./test_circles`) and sends each as a `sendVideoNote` so the admin can
/// compare what Telegram does to small / medium / max-quality inputs. Files
/// are expected to be named `test_small.mp4`, `test_medium.mp4`, and
/// `test_max.mp4`. Each video-note is preceded by a labelled text message
/// describing the encode preset, file size, and bitrate so the admin can
/// match render quality back to encoder choices when comparing.
pub async fn handle_test_circle_command(bot: &Bot, chat_id: ChatId, user_id: i64) -> Result<()> {
    if !is_admin(user_id) {
        bot.send_message(chat_id, "❌ You don't have permission to execute this command.")
            .await?;
        return Ok(());
    }

    let dir = std::env::var("TEST_CIRCLES_DIR").unwrap_or_else(|_| "./test_circles".to_string());
    let dir_path = std::path::PathBuf::from(&dir);

    let cases: &[(&str, &str, &str)] = &[
        (
            "test_small.mp4",
            "🟢 SMALL",
            "preset=veryslow, CRF=16, maxrate=1500k → 12 MB (under cloud cap)",
        ),
        (
            "test_medium.mp4",
            "🟡 MEDIUM",
            "preset=medium, CRF=14, maxrate=2800k → 21 MB (just over cap, local Bot API only)",
        ),
        (
            "test_max.mp4",
            "🔴 MAX",
            "preset=medium, CRF=12, maxrate=5000k → 34 MB (near-lossless input, local Bot API only)",
        ),
    ];

    bot.send_message(
        chat_id,
        format!(
            "🧪 Sending {} test video-notes from `{}`. Compare each one — the rendered quality on the Telegram client tells us which input survives the server re-encode best.",
            cases.len(),
            dir
        ),
    )
    .await?;

    for (filename, label, settings) in cases {
        let path = dir_path.join(filename);
        if !path.exists() {
            bot.send_message(
                chat_id,
                format!(
                    "❌ {} — file missing at `{}`. Place it there or set TEST_CIRCLES_DIR.",
                    label,
                    path.display()
                ),
            )
            .await
            .ok();
            continue;
        }
        let size_bytes = fs_err::tokio::metadata(&path).await.map(|m| m.len()).unwrap_or(0);
        let size_mb = size_bytes as f64 / 1024.0 / 1024.0;
        bot.send_message(chat_id, format!("{label} — {settings}\n📊 File: {size_mb:.1} MB"))
            .await
            .ok();

        let file = teloxide::types::InputFile::file(&path);
        match bot.send_video_note(chat_id, file).await {
            Ok(_) => {
                bot.send_message(chat_id, format!("✅ {label} sent successfully"))
                    .await
                    .ok();
            }
            Err(e) => {
                bot.send_message(chat_id, format!("❌ {label} send failed: {e}"))
                    .await
                    .ok();
            }
        }
    }

    bot.send_message(
        chat_id,
        "🧪 Test complete. Re-download each video-note from Telegram and run `ffprobe` on it — Telegram's chosen bitrate, encoder version, and any size/aspect mutation tell you what survived their pipeline.",
    )
    .await?;
    Ok(())
}

/// `/update_health_check` — manual probe of the local `/health` endpoint
/// and immediate avatar/title refresh based on the result.
///
/// Mirrors what the external `health-monitor` s6 service does on its 30 s
/// loop, but on-demand and reporting *every* failure mode (including timeouts)
/// straight back to the admin chat.
pub async fn handle_update_health_check_command(bot: &Bot, chat_id: ChatId, user_id: i64) -> Result<()> {
    if !is_admin(user_id) {
        bot.send_message(chat_id, "❌ You don't have permission to execute this command.")
            .await?;
        return Ok(());
    }

    let status = bot
        .send_message(chat_id, "🩺 Probing /health and refreshing avatar…")
        .await?;
    let status_id = status.id;

    // Probe /health with the same URL the external monitor uses.
    let health_url =
        std::env::var("HEALTH_MONITOR_HEALTH_URL").unwrap_or_else(|_| "http://localhost:9090/health".to_string());
    let probe_timeout = Duration::from_secs(10);
    let probe_started = std::time::Instant::now();

    let client = reqwest::Client::new();
    let probe_result = timeout(probe_timeout, async {
        client
            .get(&health_url)
            .timeout(probe_timeout)
            .send()
            .await
            .map_err(|e| format!("request: {e}"))
            .map(|r| {
                let status = r.status();
                (status, r)
            })
    })
    .await;

    // Each branch yields a (healthy: bool, summary: String) so we can drive
    // the avatar update *and* render a single response with full diagnostics —
    // including timeout, transport, and HTTP-level failures.
    let (healthy, probe_summary) = match probe_result {
        Err(_) => (
            false,
            format!(
                "❌ Probe timed out after {} s (url: {})",
                probe_timeout.as_secs(),
                health_url
            ),
        ),
        Ok(Err(e)) => (false, format!("❌ Probe transport error: {} (url: {})", e, health_url)),
        Ok(Ok((status_code, resp))) => {
            let body = resp.text().await.unwrap_or_default();
            let elapsed_ms = probe_started.elapsed().as_millis();
            let healthy = status_code.is_success() && body.contains("healthy");
            let body_preview: String = body.chars().take(200).collect();
            let icon = if healthy { "✅" } else { "❌" };
            (
                healthy,
                format!(
                    "{} HTTP {} in {} ms\nbody: {}",
                    icon, status_code, elapsed_ms, body_preview
                ),
            )
        }
    };

    // Run the title and avatar updates next. Each is reported separately
    // so the admin sees which exact Bot API call failed — e.g. the photo
    // hits a 12 h rate-limit while the name update succeeded, or vice
    // versa.
    let (name_result, avatar_result) = super::super::avatar::try_set_state_verbose(bot, healthy).await;
    let target = if healthy { "ONLINE" } else { "OFFLINE" };
    let name_summary = match name_result {
        Ok(()) => format!("✅ Title set to {}", target),
        Err(e) => format!("⚠️ Title update failed: {}", e),
    };
    let avatar_summary = match avatar_result {
        Ok(()) => format!("✅ Avatar set to {}", target),
        Err(e) => format!("⚠️ Avatar update failed: {}", e),
    };

    let response = format!(
        "🩺 *Manual health-check refresh*\n\n*Probe:*\n{}\n\n*Title:*\n{}\n\n*Avatar:*\n{}",
        probe_summary, name_summary, avatar_summary
    );
    bot.edit_message_text(chat_id, status_id, truncate_message(&response))
        .await
        .ok();
    Ok(())
}

pub async fn handle_downsub_health_command(
    bot: &Bot,
    chat_id: ChatId,
    user_id: i64,
    downsub_gateway: Arc<DownsubGateway>,
) -> Result<()> {
    if !is_admin(user_id) {
        bot.send_message(chat_id, "❌ You don't have permission to execute this command.")
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
