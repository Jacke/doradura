//! External health monitor for doradura bot.
//!
//! Runs as a separate s6 service. Periodically pings the bot's `/health`
//! endpoint and switches the bot avatar between online/offline on status
//! transitions. Covers crash scenarios where the bot process dies without
//! graceful shutdown.
//!
//! Single source of truth for bot name/avatar — the main bot process
//! does NOT change avatar on smoke test results.

use std::env;
use std::time::Duration;

use log::{error, info, warn};
use reqwest::Client;

const ONLINE_AVATAR: &[u8] = include_bytes!("../../../assets/avatar/online.png");
const OFFLINE_AVATAR: &[u8] = include_bytes!("../../../assets/avatar/offline.png");

const ONLINE_NAME: &str = "Dora \u{2013} Downloader Youtube Instagram TikTok";
const OFFLINE_NAME: &str = "Dora \u{2013} Sleep";

/// Always use official Telegram API — Local Bot API doesn't support
/// newer methods like setMyProfilePhoto (Bot API 9.4).
const TELEGRAM_API_URL: &str = "https://api.telegram.org";

struct Config {
    bot_token: String,
    bot_api_url: String,
    health_url: String,
    interval: Duration,
    fail_threshold: u32,
    startup_delay: Duration,
}

impl Config {
    fn from_env() -> Self {
        let bot_token = env::var("TELOXIDE_TOKEN")
            .or_else(|_| env::var("BOT_TOKEN"))
            .expect("TELOXIDE_TOKEN or BOT_TOKEN must be set");

        let bot_api_url = env::var("BOT_API_URL").unwrap_or_else(|_| "http://localhost:8081".into());

        let health_url =
            env::var("HEALTH_MONITOR_HEALTH_URL").unwrap_or_else(|_| "http://localhost:9090/health".into());

        let interval_secs: u64 = env::var("HEALTH_MONITOR_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30);

        let fail_threshold: u32 = env::var("HEALTH_MONITOR_FAIL_THRESHOLD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3);

        let startup_delay_secs: u64 = env::var("HEALTH_MONITOR_STARTUP_DELAY_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60);

        Self {
            bot_token,
            bot_api_url,
            health_url,
            interval: Duration::from_secs(interval_secs),
            fail_threshold,
            startup_delay: Duration::from_secs(startup_delay_secs),
        }
    }
}

/// Try to set bot name. Returns true on success.
async fn try_set_name(client: &Client, token: &str, name: &str) -> bool {
    let url = format!("{}/bot{}/setMyName", TELEGRAM_API_URL, token);
    info!("[API] POST setMyName name=\"{}\"", name);

    let resp = match client
        .post(&url)
        .json(&serde_json::json!({ "name": name }))
        .timeout(Duration::from_secs(10))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            error!("[API] setMyName request failed: {e}");
            return false;
        }
    };

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    info!("[API] setMyName response: HTTP {} | {:.200}", status, body);

    if status.is_success() && body.contains("\"ok\":true") {
        info!("[API] setMyName: success");
        return true;
    }
    if status.as_u16() == 429 {
        warn!("[API] setMyName: 429 — will retry next cycle");
    } else {
        warn!("[API] setMyName: HTTP {} — will retry next cycle", status);
    }
    false
}

/// Try to set bot avatar. Returns true on success.
async fn try_set_avatar(client: &Client, _api_url: &str, token: &str, photo: &[u8]) -> bool {
    let photo_file = match reqwest::multipart::Part::bytes(photo.to_vec())
        .file_name("photo.png")
        .mime_str("image/png")
    {
        Ok(p) => p,
        Err(e) => {
            error!("[API] Failed to build multipart: {e}");
            return false;
        }
    };

    let form = reqwest::multipart::Form::new()
        .text("photo", r#"{"type":"static","photo":"attach://photo_file"}"#)
        .part("photo_file", photo_file);

    let url = format!("{}/bot{}/setMyProfilePhoto", TELEGRAM_API_URL, token);
    info!("[API] POST setMyProfilePhoto (photo_size={}B)", photo.len());

    let resp = match client
        .post(&url)
        .multipart(form)
        .timeout(Duration::from_secs(30))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            error!("[API] setMyProfilePhoto request failed: {e}");
            return false;
        }
    };

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    info!("[API] setMyProfilePhoto response: HTTP {} | {:.200}", status, body);

    if status.is_success() && body.contains("\"ok\":true") {
        info!("[API] setMyProfilePhoto: success");
        return true;
    }
    if status.as_u16() == 429 {
        warn!("[API] setMyProfilePhoto: 429 — will retry next cycle");
    } else {
        warn!("[API] setMyProfilePhoto: HTTP {} — will retry next cycle", status);
    }
    false
}

async fn check_health(client: &Client, health_url: &str) -> bool {
    match client.get(health_url).timeout(Duration::from_secs(10)).send().await {
        Ok(resp) if resp.status().is_success() => match resp.text().await {
            Ok(body) => body.contains("healthy"),
            Err(_) => false,
        },
        _ => false,
    }
}

/// Desired state based on health checks.
#[derive(Clone, Copy, PartialEq)]
enum DesiredState {
    Online,
    Offline,
}

/// What we've actually set via API.
#[derive(Clone, Copy, PartialEq)]
enum ActualState {
    Unknown,
    Online,
    Offline,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    env_logger::init();

    let config = Config::from_env();

    info!(
        "Health monitor starting (interval={}s, threshold={}, startup_delay={}s, health_url={})",
        config.interval.as_secs(),
        config.fail_threshold,
        config.startup_delay.as_secs(),
        config.health_url,
    );

    let client = Client::new();

    let mut actual_name = ActualState::Unknown;
    let mut actual_avatar = ActualState::Unknown;

    // ── Startup: wait for bot, set initial state ──
    info!("Waiting {}s for bot startup...", config.startup_delay.as_secs());
    tokio::time::sleep(config.startup_delay).await;

    if check_health(&client, &config.health_url).await {
        info!("Bot is healthy after startup delay — setting ONLINE profile");
        if try_set_name(&client, &config.bot_token, ONLINE_NAME).await {
            actual_name = ActualState::Online;
        }
        if try_set_avatar(&client, &config.bot_api_url, &config.bot_token, ONLINE_AVATAR).await {
            actual_avatar = ActualState::Online;
        }
    } else {
        info!("Bot not healthy after startup, setting OFFLINE name");
        if try_set_name(&client, &config.bot_token, OFFLINE_NAME).await {
            actual_name = ActualState::Offline;
        }
        if try_set_avatar(&client, &config.bot_api_url, &config.bot_token, OFFLINE_AVATAR).await {
            actual_avatar = ActualState::Offline;
        }
    }
    info!("Startup complete, beginning health checks");

    let mut failures: u32 = config.fail_threshold;

    loop {
        let healthy = check_health(&client, &config.health_url).await;

        let desired = if healthy {
            if failures > 0 {
                info!("Health check passed (was at {failures} failures)");
            }
            failures = 0;
            DesiredState::Online
        } else {
            failures = failures.saturating_add(1);
            if failures <= config.fail_threshold {
                warn!("Health check failed ({failures}/{})", config.fail_threshold);
            }
            if failures >= config.fail_threshold {
                DesiredState::Offline
            } else {
                tokio::time::sleep(config.interval).await;
                continue;
            }
        };

        // ── Set NAME ──
        let desired_name_state = match desired {
            DesiredState::Online => ActualState::Online,
            DesiredState::Offline => ActualState::Offline,
        };

        if actual_name != desired_name_state {
            let name = match desired {
                DesiredState::Online => ONLINE_NAME,
                DesiredState::Offline => OFFLINE_NAME,
            };
            if try_set_name(&client, &config.bot_token, name).await {
                actual_name = desired_name_state;
                info!("Bot name updated successfully");
            }
            // If 429, actual_name stays unchanged → will retry next cycle
        }

        // ── Set AVATAR ──
        if actual_avatar != desired_name_state {
            let photo = match desired {
                DesiredState::Online => ONLINE_AVATAR,
                DesiredState::Offline => OFFLINE_AVATAR,
            };
            if try_set_avatar(&client, &config.bot_api_url, &config.bot_token, photo).await {
                actual_avatar = desired_name_state;
                info!("Bot avatar updated successfully");
            }
        }

        tokio::time::sleep(config.interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_monitor_compiles() {
        // Smoke test — ensure the module compiles
        assert_eq!(ONLINE_NAME, "Dora \u{2013} Downloader Youtube Instagram TikTok");
        assert_eq!(OFFLINE_NAME, "Dora \u{2013} Sleep");
    }
}
