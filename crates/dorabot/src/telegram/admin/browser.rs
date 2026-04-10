use super::{escape_markdown, is_admin};
use crate::telegram::Bot;
use anyhow::Result;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, MessageId, ParseMode};

/// Default cookie manager API URL
const COOKIE_MANAGER_URL: &str = "http://127.0.0.1:9876";

/// Send an HTTP request to the cookie_manager.py API
pub(super) async fn cookie_manager_request(method: &str, path: &str) -> anyhow::Result<serde_json::Value> {
    let url = format!("{}{}", COOKIE_MANAGER_URL, path);
    // login_start launches Xvfb + Chromium + VNC — needs more time
    let timeout_secs = if path.contains("login_start") { 90 } else { 30 };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()?;

    let response = match method {
        "POST" => client.post(&url).send().await,
        _ => client.get(&url).send().await,
    };

    let resp = response?;
    let status = resp.status();
    let body = resp.text().await?;

    if !status.is_success() {
        anyhow::bail!("HTTP {}: {}", status, body);
    }

    let val: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| anyhow::anyhow!("JSON parse error: {} (body: {})", e, body))?;
    Ok(val)
}

/// Handles the /browser_login command (admin only)
///
/// Starts a visual login session via noVNC so the admin can log in to YouTube.
pub async fn handle_browser_login_command(bot: &Bot, chat_id: ChatId, user_id: i64) -> Result<()> {
    if !is_admin(user_id) {
        bot.send_message(chat_id, "❌ Only admins can use this command.")
            .await?;
        return Ok(());
    }

    log::info!("Admin {} starting browser login session", user_id);

    let msg = bot
        .send_message(chat_id, "🔄 Starting browser login session...")
        .await?;

    match cookie_manager_request("POST", "/api/login_start").await {
        Ok(data) => {
            if let Some(error) = data.get("error").and_then(|v| v.as_str()) {
                bot.edit_message_text(chat_id, msg.id, format!("❌ {}", error)).await?;
                return Ok(());
            }

            let novnc_url = data.get("novnc_url").and_then(|v| v.as_str()).unwrap_or("unknown");

            let escaped_url = escape_markdown(novnc_url);

            // Build keyboard: only add URL button if it's a valid public URL
            let is_public_url = novnc_url.starts_with("https://")
                || (novnc_url.starts_with("http://")
                    && !novnc_url.contains("localhost")
                    && !novnc_url.contains("127.0.0.1"));

            let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
            if is_public_url {
                if let Ok(url) = novnc_url.parse() {
                    rows.push(vec![InlineKeyboardButton::url("🌐 Open noVNC", url)]);
                }
            }
            rows.push(vec![
                crate::telegram::cb("✅ Done — export cookies", "admin:browser_login_done".to_string()),
                crate::telegram::cb("❌ Cancel", "admin:browser_login_cancel".to_string()),
            ]);
            let keyboard = InlineKeyboardMarkup::new(rows);

            bot.edit_message_text(
                chat_id,
                msg.id,
                format!(
                    "🌐 *Browser login session started*\n\n\
                     Open the link below to log in to YouTube:\n\
                     `{}`\n\n\
                     After logging in, press *Done* to export cookies\\.",
                    escaped_url
                ),
            )
            .parse_mode(ParseMode::MarkdownV2)
            .reply_markup(keyboard)
            .await?;
        }
        Err(e) => {
            bot.edit_message_text(chat_id, msg.id, format!("❌ Failed to start login session: {}", e))
                .await?;
        }
    }

    Ok(())
}

/// Handles the /browser_status command (admin only)
///
/// Shows the current cookie manager status.
pub async fn handle_browser_status_command(bot: &Bot, chat_id: ChatId, user_id: i64) -> Result<()> {
    if !is_admin(user_id) {
        bot.send_message(chat_id, "❌ Only admins can use this command.")
            .await?;
        return Ok(());
    }

    match cookie_manager_request("GET", "/api/status").await {
        Ok(data) => {
            let login_active = data.get("login_active").and_then(|v| v.as_bool()).unwrap_or(false);
            let needs_relogin = data.get("needs_relogin").and_then(|v| v.as_bool()).unwrap_or(false);
            let profile_exists = data.get("profile_exists").and_then(|v| v.as_bool()).unwrap_or(false);
            let cookies_exist = data.get("cookies_exist").and_then(|v| v.as_bool()).unwrap_or(false);
            let cookie_count = data.get("cookie_count").and_then(|v| v.as_u64()).unwrap_or(0);
            let last_refresh = data.get("last_refresh").and_then(|v| v.as_str()).unwrap_or("never");
            let last_success = data.get("last_refresh_success").and_then(|v| v.as_bool());
            let last_error = data.get("last_error").and_then(|v| v.as_str());

            // Persistent browser status
            let browser_running = data.get("browser_running").and_then(|v| v.as_bool()).unwrap_or(false);
            let browser_restarts = data.get("browser_restarts").and_then(|v| v.as_u64()).unwrap_or(0);
            let browser_memory_mb = data.get("browser_memory_mb").and_then(|v| v.as_u64());

            // Get detailed cookie analysis
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
            let invalid_reason = data.get("invalid_reason").and_then(|v| v.as_str());

            let status_icon = if needs_relogin {
                "🔴"
            } else if cookies_exist && cookie_count > 0 {
                "🟢"
            } else {
                "🟡"
            };

            let browser_status = if browser_running {
                let mem_info = browser_memory_mb.map(|m| format!(" \\({}MB\\)", m)).unwrap_or_default();
                format!("🟢 Running{}", mem_info)
            } else {
                "🔴 Not running".to_string()
            };

            let login_status = if login_active {
                "🌐 Active login session"
            } else {
                "— No active session"
            };

            let refresh_icon = match last_success {
                Some(true) => "✅",
                Some(false) => "❌",
                None => "—",
            };

            let escaped_refresh = escape_markdown(last_refresh);
            let error_line = if let Some(err) = last_error {
                // Inside backticks, MarkdownV2 only needs ` and \ escaped
                let escaped_err = err.replace('\\', "\\\\").replace('`', "\\`");
                format!("\n⚠️ Last error: `{}`", escaped_err)
            } else {
                String::new()
            };

            // Build session cookies detail
            let session_detail = if needs_relogin {
                let missing_str = if required_missing.is_empty() {
                    "none".to_string()
                } else {
                    escape_markdown(&required_missing.join(", "))
                };
                let found_str = if required_found.is_empty() {
                    "none".to_string()
                } else {
                    escape_markdown(&required_found.join(", "))
                };
                let reason_str = if let Some(reason) = invalid_reason {
                    format!("\n❗ _{}_", escape_markdown(reason))
                } else {
                    String::new()
                };
                format!(
                    "\n\n*Session cookies:*\n✅ Found: {}\n❌ Missing: {}{}",
                    found_str, missing_str, reason_str
                )
            } else {
                let found_str = if required_found.is_empty() {
                    "checking\\.\\.\\.".to_string()
                } else {
                    escape_markdown(&required_found.join(", "))
                };
                format!("\n\n*Session cookies:* ✅ {}", found_str)
            };

            let mut buttons = vec![];
            if needs_relogin || !profile_exists {
                buttons.push(vec![crate::telegram::cb(
                    "🌐 Start login",
                    "admin:browser_login_start".to_string(),
                )]);
            }
            buttons.push(vec![
                crate::telegram::cb("🔄 Refresh", "admin:browser_force_refresh".to_string()),
                crate::telegram::cb("🔃 Restart browser", "admin:browser_restart".to_string()),
            ]);

            let keyboard = InlineKeyboardMarkup::new(buttons);

            let restarts_info = if browser_restarts > 0 {
                format!(" \\({} restarts\\)", browser_restarts)
            } else {
                String::new()
            };

            bot.send_message(
                chat_id,
                format!(
                    "{} *Cookie Manager Status*\n\n\
                     🌐 Browser: {}{}\n\
                     Profile: {}\n\
                     Cookies: {} \\({} cookies\\)\n\
                     Login: {}\n\
                     Last refresh: {} {}\
                     {}{}\n\n\
                     _Needs re\\-login: {}_",
                    status_icon,
                    browser_status,
                    restarts_info,
                    if profile_exists { "✅ exists" } else { "❌ missing" },
                    if cookies_exist { "✅" } else { "❌" },
                    cookie_count,
                    login_status,
                    refresh_icon,
                    escaped_refresh,
                    error_line,
                    session_detail,
                    if needs_relogin { "yes" } else { "no" },
                ),
            )
            .parse_mode(ParseMode::MarkdownV2)
            .reply_markup(keyboard)
            .await?;
        }
        Err(e) => {
            bot.send_message(
                chat_id,
                format!(
                    "❌ Cookie manager is not reachable: {}\n\nMake sure it's running on {}",
                    e, COOKIE_MANAGER_URL
                ),
            )
            .await?;
        }
    }

    Ok(())
}

/// Handles admin:browser_* callback queries
pub async fn handle_browser_callback(
    bot: &Bot,
    _callback_id: String,
    chat_id: ChatId,
    message_id: MessageId,
    data: &str,
) -> Result<()> {
    match data {
        "admin:browser_login_done" => {
            bot.edit_message_text(chat_id, message_id, "🔄 Exporting cookies...")
                .await?;

            match cookie_manager_request("POST", "/api/login_stop").await {
                Ok(resp) => {
                    let exported = resp.get("cookies_exported").and_then(|v| v.as_bool()).unwrap_or(false);
                    let count = resp.get("cookie_count").and_then(|v| v.as_u64()).unwrap_or(0);

                    if exported && count > 0 {
                        bot.edit_message_text(
                            chat_id,
                            message_id,
                            format!("✅ Login complete! Exported {} cookies.", count),
                        )
                        .await?;
                    } else {
                        bot.edit_message_text(
                            chat_id,
                            message_id,
                            format!(
                                "⚠️ Login stopped. Cookies exported: {}, count: {}.\n\
                                 Try /browser_login again if cookies seem insufficient.",
                                exported, count
                            ),
                        )
                        .await?;
                    }
                }
                Err(e) => {
                    // Request failed (likely timeout), but cookies might have been exported
                    if let Ok(status) = cookie_manager_request("GET", "/api/status").await {
                        let cookie_count = status.get("cookie_count").and_then(|v| v.as_u64()).unwrap_or(0);
                        if cookie_count > 0 {
                            bot.edit_message_text(
                                chat_id,
                                message_id,
                                format!("✅ Login complete! Exported {} cookies.\n(Response was slow but operation succeeded)", cookie_count),
                            )
                            .await?;
                            return Ok(());
                        }
                    }
                    bot.edit_message_text(chat_id, message_id, format!("❌ Failed to export cookies: {}", e))
                        .await?;
                }
            }
        }

        "admin:browser_login_cancel" => {
            let _ = cookie_manager_request("POST", "/api/login_stop").await;
            bot.edit_message_text(chat_id, message_id, "❌ Login session cancelled.")
                .await?;
        }

        "admin:browser_login_start" => {
            bot.edit_message_text(chat_id, message_id, "🔄 Starting login session...")
                .await?;

            match cookie_manager_request("POST", "/api/login_start").await {
                Ok(data) => {
                    if let Some(error) = data.get("error").and_then(|v| v.as_str()) {
                        bot.edit_message_text(chat_id, message_id, format!("❌ {}", error))
                            .await?;
                        return Ok(());
                    }

                    let novnc_url = data.get("novnc_url").and_then(|v| v.as_str()).unwrap_or("unknown");

                    let escaped_url = escape_markdown(novnc_url);

                    let keyboard = InlineKeyboardMarkup::new(vec![
                        vec![InlineKeyboardButton::url(
                            "🌐 Open noVNC",
                            novnc_url
                                .parse()
                                .unwrap_or_else(|_| "https://example.com".parse().unwrap()),
                        )],
                        vec![
                            crate::telegram::cb("✅ Done — export cookies", "admin:browser_login_done".to_string()),
                            crate::telegram::cb("❌ Cancel", "admin:browser_login_cancel".to_string()),
                        ],
                    ]);

                    bot.edit_message_text(
                        chat_id,
                        message_id,
                        format!(
                            "🌐 *Browser login session started*\n\n\
                             Open the link below to log in to YouTube:\n\
                             `{}`\n\n\
                             After logging in, press *Done* to export cookies\\.",
                            escaped_url
                        ),
                    )
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(keyboard)
                    .await?;
                }
                Err(e) => {
                    bot.edit_message_text(chat_id, message_id, format!("❌ Failed to start login session: {}", e))
                        .await?;
                }
            }
        }

        "admin:browser_force_refresh" => {
            bot.edit_message_text(chat_id, message_id, "🔄 Force refreshing cookies...")
                .await?;

            match cookie_manager_request("POST", "/api/export_cookies").await {
                Ok(resp) => {
                    let success = resp.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
                    let count = resp.get("cookie_count").and_then(|v| v.as_u64()).unwrap_or(0);

                    if success {
                        bot.edit_message_text(
                            chat_id,
                            message_id,
                            format!("✅ Cookies refreshed! {} cookies exported.", count),
                        )
                        .await?;
                    } else {
                        let error = resp.get("error").and_then(|v| v.as_str()).unwrap_or("unknown error");
                        bot.edit_message_text(chat_id, message_id, format!("❌ Refresh failed: {}", error))
                            .await?;
                    }
                }
                Err(e) => {
                    bot.edit_message_text(chat_id, message_id, format!("❌ Failed to refresh cookies: {}", e))
                        .await?;
                }
            }
        }

        "admin:browser_restart" => {
            bot.edit_message_text(chat_id, message_id, "🔃 Restarting browser...")
                .await?;

            match cookie_manager_request("POST", "/api/restart_browser").await {
                Ok(resp) => {
                    let success = resp.get("success").and_then(|v| v.as_bool()).unwrap_or(false);

                    if success {
                        bot.edit_message_text(
                            chat_id,
                            message_id,
                            "✅ Browser restarted successfully.\n\nUse /browser_status to check current state.",
                        )
                        .await?;
                    } else {
                        let error = resp.get("error").and_then(|v| v.as_str()).unwrap_or("unknown error");
                        bot.edit_message_text(chat_id, message_id, format!("❌ Restart failed: {}", error))
                            .await?;
                    }
                }
                Err(e) => {
                    bot.edit_message_text(chat_id, message_id, format!("❌ Failed to restart browser: {}", e))
                        .await?;
                }
            }
        }

        _ => {
            log::warn!("Unknown browser callback: {}", data);
        }
    }

    Ok(())
}

/// Shows proxy statistics and health status
pub async fn handle_proxy_stats_command(bot: &Bot, chat_id: ChatId, _user_id: i64) -> Result<()> {
    use crate::core::config;
    use crate::download::proxy::ProxyListManager;

    if config::proxy::WARP_PROXY.is_none() && config::proxy::PROXY_FILE.is_none() {
        bot.send_message(
            chat_id,
            "❌ *No proxies configured*\n\nSet WARP_PROXY or PROXY_FILE environment variables.",
        )
        .parse_mode(ParseMode::MarkdownV2)
        .await?;
        return Ok(());
    }

    let manager = ProxyListManager::new(config::proxy::get_selection_strategy());
    let stats = manager.all_stats().await;

    if stats.is_empty() {
        bot.send_message(chat_id, "ℹ️ *Proxy system configured but no proxies loaded yet*")
            .parse_mode(ParseMode::MarkdownV2)
            .await?;
        return Ok(());
    }

    let mut message = "🔄 *Proxy Statistics*\n\n".to_string();
    message.push_str(&format!("Strategy: `{}`\n", config::proxy::PROXY_STRATEGY.as_str()));
    message.push_str(&format!("Min Health: `{:.1}%`\n", *config::proxy::MIN_HEALTH * 100.0));
    message.push_str(&format!("Total Proxies: `{}`\n\n", stats.len()));

    message.push_str("*Proxy Health:*\n");
    for (proxy_url, stat) in stats.iter().take(10) {
        let total = stat.successes + stat.failures;
        let success_rate = if total > 0 {
            stat.successes as f64 / total as f64
        } else {
            0.0
        };

        let health_emoji = if success_rate >= 0.9 {
            "✅"
        } else if success_rate >= 0.7 {
            "⚠️ "
        } else {
            "❌"
        };

        message.push_str(&format!(
            "{} `{:.0}%` \\| {} ok\\, {} err\n",
            health_emoji,
            success_rate * 100.0,
            stat.successes,
            stat.failures
        ));
        if proxy_url.chars().count() > 40 {
            message.push_str(&format!("`{}...`\n", proxy_url.chars().take(37).collect::<String>()));
        } else {
            message.push_str(&format!("`{}`\n", proxy_url));
        }
    }

    if stats.len() > 10 {
        message.push_str(&format!("\n_... and {} more proxies_", stats.len() - 10));
    }

    bot.send_message(chat_id, message)
        .parse_mode(ParseMode::MarkdownV2)
        .await?;

    Ok(())
}

/// Resets proxy health statistics
pub async fn handle_proxy_reset_command(bot: &Bot, chat_id: ChatId, _user_id: i64) -> Result<()> {
    use crate::core::config;
    use crate::download::proxy::ProxyListManager;

    if config::proxy::WARP_PROXY.is_none() && config::proxy::PROXY_FILE.is_none() {
        bot.send_message(chat_id, "❌ *No proxies configured*")
            .parse_mode(ParseMode::MarkdownV2)
            .await?;
        return Ok(());
    }

    let manager = ProxyListManager::new(config::proxy::get_selection_strategy());
    manager.reset_stats().await;

    bot.send_message(
        chat_id,
        "✅ *Proxy statistics reset*\n\nAll health counters have been cleared.",
    )
    .parse_mode(ParseMode::MarkdownV2)
    .await?;

    Ok(())
}
