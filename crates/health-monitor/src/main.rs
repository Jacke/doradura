//! External health monitor for doradura bot.
//!
//! Runs as a separate s6 service. Periodically pings the bot's `/health`
//! endpoint and switches the bot avatar between online/offline on status
//! transitions. Covers crash scenarios where the bot process dies without
//! graceful shutdown.

use std::env;
use std::time::Duration;

use log::{error, info, warn};
use reqwest::Client;

const ONLINE_AVATAR: &[u8] = include_bytes!("../../../assets/avatar/online.png");
const OFFLINE_AVATAR: &[u8] = include_bytes!("../../../assets/avatar/offline.png");

/// Always use official Telegram API for avatar changes — Local Bot API
/// often doesn't support newer methods like setMyProfilePhoto (Bot API 8.2+).
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

async fn set_avatar(client: &Client, _api_url: &str, token: &str, photo: &[u8]) -> Result<(), String> {
    let photo_part = reqwest::multipart::Part::bytes(photo.to_vec())
        .file_name("photo.png")
        .mime_str("image/png")
        .map_err(|e| e.to_string())?;

    let form = reqwest::multipart::Form::new().part("photo", photo_part);

    // Use official Telegram API — Local Bot API doesn't support setMyProfilePhoto
    let url = format!("{}/bot{}/setMyProfilePhoto", TELEGRAM_API_URL, token);

    let resp: serde_json::Value = client
        .post(&url)
        .multipart(form)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("json parse failed: {e}"))?;

    if resp.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        Ok(())
    } else {
        let desc = resp
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        Err(format!("Bot API error: {desc}"))
    }
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

    // Wait for bot to start up before monitoring
    info!("Waiting {}s for bot startup...", config.startup_delay.as_secs());
    tokio::time::sleep(config.startup_delay).await;
    info!("Startup delay complete, beginning health checks");

    let client = Client::new();
    let mut is_online = false;
    let mut failures: u32 = config.fail_threshold; // start assuming bot is down
    let mut avatar_errors: u32 = 0; // backoff counter for avatar API failures

    loop {
        let healthy = check_health(&client, &config.health_url).await;

        if healthy {
            if failures > 0 {
                info!("Health check passed (was at {failures} failures)");
            }
            failures = 0;

            if !is_online {
                // Backoff: only retry avatar after 2^n intervals (max ~30 min)
                let backoff_intervals = 1u32.checked_shl(avatar_errors.min(6)).unwrap_or(64);
                if avatar_errors == 0 || failures == 0 {
                    info!("Bot is healthy — setting ONLINE avatar");
                    match set_avatar(&client, &config.bot_api_url, &config.bot_token, ONLINE_AVATAR).await {
                        Ok(()) => {
                            is_online = true;
                            avatar_errors = 0;
                            info!("Online avatar set successfully");
                        }
                        Err(e) => {
                            avatar_errors += 1;
                            error!(
                                "Failed to set online avatar: {e} (will retry in ~{}s)",
                                backoff_intervals * config.interval.as_secs() as u32
                            );
                        }
                    }
                } else {
                    // Skip this cycle, wait for backoff
                    avatar_errors = avatar_errors.saturating_sub(1);
                }
            }
        } else {
            failures = failures.saturating_add(1);
            warn!("Health check failed ({failures}/{})", config.fail_threshold);

            if failures >= config.fail_threshold && is_online {
                warn!("Threshold reached — setting OFFLINE avatar");
                match set_avatar(&client, &config.bot_api_url, &config.bot_token, OFFLINE_AVATAR).await {
                    Ok(()) => {
                        is_online = false;
                        avatar_errors = 0;
                        info!("Offline avatar set successfully");
                    }
                    Err(e) => error!("Failed to set offline avatar: {e}"),
                }
            }
        }

        tokio::time::sleep(config.interval).await;
    }
}
