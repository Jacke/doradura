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
use std::time::{Duration, Instant};

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

/// Result of a Telegram API call that may be rate-limited.
enum ApiResult {
    Ok,
    /// Rate-limited — must wait this many seconds before next call.
    RateLimited(u64),
    /// Other error (network, API error, etc.)
    Error(String),
}

/// Parse `retry_after` from Telegram error description like
/// "Too Many Requests: retry after 13317"
fn parse_retry_after(description: &str) -> Option<u64> {
    if description.contains("retry after") {
        description
            .rsplit("retry after ")
            .next()
            .and_then(|s| s.trim().parse::<u64>().ok())
    } else {
        None
    }
}

async fn set_avatar(client: &Client, _api_url: &str, token: &str, photo: &[u8]) -> ApiResult {
    let photo_file = match reqwest::multipart::Part::bytes(photo.to_vec())
        .file_name("photo.png")
        .mime_str("image/png")
    {
        Ok(p) => p,
        Err(e) => return ApiResult::Error(e.to_string()),
    };

    let form = reqwest::multipart::Form::new()
        .text("photo", r#"{"type":"static","photo":"attach://photo_file"}"#)
        .part("photo_file", photo_file);

    let url = format!("{}/bot{}/setMyProfilePhoto", TELEGRAM_API_URL, token);

    let resp: serde_json::Value = match client
        .post(&url)
        .multipart(form)
        .timeout(Duration::from_secs(30))
        .send()
        .await
    {
        Ok(r) => match r.json().await {
            Ok(j) => j,
            Err(e) => return ApiResult::Error(format!("json parse failed: {e}")),
        },
        Err(e) => return ApiResult::Error(format!("request failed: {e}")),
    };

    if resp.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        return ApiResult::Ok;
    }

    let desc = resp
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown error");

    if let Some(retry_after) = parse_retry_after(desc) {
        ApiResult::RateLimited(retry_after)
    } else {
        ApiResult::Error(format!("Bot API error: {desc}"))
    }
}

async fn set_bot_name(client: &Client, token: &str, name: &str) -> ApiResult {
    let url = format!("{}/bot{}/setMyName", TELEGRAM_API_URL, token);

    let resp: serde_json::Value = match client
        .post(&url)
        .json(&serde_json::json!({ "name": name }))
        .timeout(Duration::from_secs(10))
        .send()
        .await
    {
        Ok(r) => match r.json().await {
            Ok(j) => j,
            Err(e) => return ApiResult::Error(format!("json parse failed: {e}")),
        },
        Err(e) => return ApiResult::Error(format!("request failed: {e}")),
    };

    if resp.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        return ApiResult::Ok;
    }

    let desc = resp
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown error");

    if let Some(retry_after) = parse_retry_after(desc) {
        ApiResult::RateLimited(retry_after)
    } else {
        ApiResult::Error(format!("setMyName error: {desc}"))
    }
}

/// Apply a rate-limit cooldown. Returns the new `rate_limit_until` if
/// `retry_secs` extends beyond the current deadline.
fn apply_rate_limit(current: Option<Instant>, retry_secs: u64) -> Option<Instant> {
    let new_deadline = Instant::now() + Duration::from_secs(retry_secs);
    match current {
        Some(existing) if existing > new_deadline => Some(existing),
        _ => Some(new_deadline),
    }
}

fn is_rate_limited(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|d| Instant::now() < d)
}

fn rate_limit_remaining_secs(deadline: Option<Instant>) -> u64 {
    deadline
        .map(|d| d.saturating_duration_since(Instant::now()).as_secs())
        .unwrap_or(0)
}

/// Try to set bot name. Returns true if successful.
async fn try_set_name(client: &Client, token: &str, name: &str, rate_limit_until: &mut Option<Instant>) -> bool {
    match set_bot_name(client, token, name).await {
        ApiResult::Ok => true,
        ApiResult::RateLimited(secs) => {
            warn!(
                "setMyName rate-limited for {}s ({:.1}h) — will wait",
                secs,
                secs as f64 / 3600.0
            );
            *rate_limit_until = apply_rate_limit(*rate_limit_until, secs);
            false
        }
        ApiResult::Error(e) => {
            warn!("Failed to set bot name: {e}");
            false
        }
    }
}

/// Try to set bot avatar. Returns true if successful.
async fn try_set_avatar(
    client: &Client,
    api_url: &str,
    token: &str,
    photo: &[u8],
    rate_limit_until: &mut Option<Instant>,
) -> bool {
    match set_avatar(client, api_url, token, photo).await {
        ApiResult::Ok => true,
        ApiResult::RateLimited(secs) => {
            warn!(
                "setMyProfilePhoto rate-limited for {}s ({:.1}h) — will wait",
                secs,
                secs as f64 / 3600.0
            );
            *rate_limit_until = apply_rate_limit(*rate_limit_until, secs);
            false
        }
        ApiResult::Error(e) => {
            error!("Failed to set avatar: {e}");
            false
        }
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

    // Global rate-limit deadline — skip ALL profile API calls until this time.
    let mut rate_limit_until: Option<Instant> = None;

    // Track actual state of name and avatar independently.
    let mut actual_name = ActualState::Unknown;
    let mut actual_avatar = ActualState::Unknown;

    // ── Startup: set offline name (skip avatar to conserve rate limit) ──
    info!("Setting OFFLINE name on startup (bot not ready yet)");
    if try_set_name(&client, &config.bot_token, OFFLINE_NAME, &mut rate_limit_until).await {
        actual_name = ActualState::Offline;
    }
    // Try avatar too, but don't block on failure
    if !is_rate_limited(rate_limit_until)
        && try_set_avatar(
            &client,
            &config.bot_api_url,
            &config.bot_token,
            OFFLINE_AVATAR,
            &mut rate_limit_until,
        )
        .await
    {
        actual_avatar = ActualState::Offline;
    }

    // Wait for bot to start up before monitoring
    info!("Waiting {}s for bot startup...", config.startup_delay.as_secs());
    tokio::time::sleep(config.startup_delay).await;
    info!("Startup delay complete, beginning health checks");

    let mut failures: u32 = config.fail_threshold; // start assuming bot is down

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
                // Not enough failures yet — keep current state, don't change anything
                tokio::time::sleep(config.interval).await;
                continue;
            }
        };

        // Check rate limit before any API call
        if is_rate_limited(rate_limit_until) {
            let remaining = rate_limit_remaining_secs(rate_limit_until);
            if remaining > 60 {
                // Only log once per minute-ish to avoid spam
                info!(
                    "Rate-limited, skipping profile update ({remaining}s / {:.1}h remaining)",
                    remaining as f64 / 3600.0
                );
            }
            tokio::time::sleep(config.interval).await;
            continue;
        }

        // ── Set NAME first (lightweight, rarely rate-limited) ──
        let desired_name_state = match desired {
            DesiredState::Online => ActualState::Online,
            DesiredState::Offline => ActualState::Offline,
        };

        if actual_name != desired_name_state {
            let name = match desired {
                DesiredState::Online => ONLINE_NAME,
                DesiredState::Offline => OFFLINE_NAME,
            };
            info!(
                "Setting bot name to {:?}",
                if desired == DesiredState::Online {
                    "online"
                } else {
                    "offline"
                }
            );
            if try_set_name(&client, &config.bot_token, name, &mut rate_limit_until).await {
                actual_name = desired_name_state;
                info!("Bot name updated successfully");
            }
            // If rate-limited, skip avatar too
            if is_rate_limited(rate_limit_until) {
                tokio::time::sleep(config.interval).await;
                continue;
            }
        }

        // ── Set AVATAR second (heavy, strict rate limits) ──
        let desired_avatar_state = desired_name_state;

        if actual_avatar != desired_avatar_state {
            let photo = match desired {
                DesiredState::Online => ONLINE_AVATAR,
                DesiredState::Offline => OFFLINE_AVATAR,
            };
            info!(
                "Setting bot avatar to {:?}",
                if desired == DesiredState::Online {
                    "online"
                } else {
                    "offline"
                }
            );
            if try_set_avatar(
                &client,
                &config.bot_api_url,
                &config.bot_token,
                photo,
                &mut rate_limit_until,
            )
            .await
            {
                actual_avatar = desired_avatar_state;
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
    fn test_parse_retry_after() {
        assert_eq!(parse_retry_after("Too Many Requests: retry after 13317"), Some(13317));
        assert_eq!(parse_retry_after("Too Many Requests: retry after 5"), Some(5));
        assert_eq!(parse_retry_after("Bad Request: invalid photo"), None);
        assert_eq!(parse_retry_after("unknown error"), None);
    }

    #[test]
    fn test_apply_rate_limit() {
        // New deadline extends beyond none
        let result = apply_rate_limit(None, 100);
        assert!(result.is_some());

        // Longer existing deadline wins
        let far_future = Some(Instant::now() + Duration::from_secs(10000));
        let result = apply_rate_limit(far_future, 5);
        assert_eq!(result, far_future);

        // Shorter existing deadline loses to new longer one
        let near_instant = Instant::now() + Duration::from_secs(1);
        let result = apply_rate_limit(Some(near_instant), 10000);
        assert!(result.unwrap() > near_instant);
    }

    #[test]
    fn test_is_rate_limited() {
        assert!(!is_rate_limited(None));
        assert!(is_rate_limited(Some(Instant::now() + Duration::from_secs(100))));
        // Past deadline = not limited
        assert!(!is_rate_limited(Some(Instant::now() - Duration::from_secs(1))));
    }
}
