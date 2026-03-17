//! Bot avatar & name status indicator.
//!
//! Changes the bot's profile photo and display name to reflect online/offline status.
//! Uses raw Bot API calls since teloxide doesn't support `setMyProfilePhoto` / `setMyName`.
//!
//! NOTE: Avatar/name transitions are primarily managed by the external health-monitor
//! s6 service. The bot only sets online status at startup. If rate-limited, it logs
//! a warning and continues — health-monitor will retry later.

use super::Bot;

const ONLINE_AVATAR: &[u8] = include_bytes!("../../../../assets/avatar/online.png");
const OFFLINE_AVATAR: &[u8] = include_bytes!("../../../../assets/avatar/offline.png");

const ONLINE_NAME: &str = "Dora – Downloader Youtube Instagram TikTok";
const OFFLINE_NAME: &str = "Dora – Sleep";
const STAGING_NAME: &str = "Dora at Rehearsal";

/// Always use official Telegram API for profile methods — Local Bot API
/// often doesn't support newer methods like setMyProfilePhoto (Bot API 8.2+).
const TELEGRAM_API: &str = "https://api.telegram.org";

fn api_url(bot: &Bot, method: &str) -> String {
    format!("{}/bot{}/{}", TELEGRAM_API, bot.token(), method)
}

/// Parse `retry_after` seconds from Telegram error description.
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

/// Set bot profile photo from PNG bytes.
///
/// Bot API 9.4: `setMyProfilePhoto` expects an `InputProfilePhotoStatic` JSON object
/// with the actual file sent as a separate multipart part via `attach://` reference.
async fn set_bot_avatar(bot: &Bot, photo_bytes: &[u8]) -> anyhow::Result<()> {
    let photo_file = reqwest::multipart::Part::bytes(photo_bytes.to_vec())
        .file_name("photo.png")
        .mime_str("image/png")
        .unwrap();

    let form = reqwest::multipart::Form::new()
        .text("photo", r#"{"type":"static","photo":"attach://photo_file"}"#)
        .part("photo_file", photo_file);

    let resp: serde_json::Value = bot
        .client()
        .post(api_url(bot, "setMyProfilePhoto"))
        .multipart(form)
        .send()
        .await?
        .json()
        .await?;

    if resp.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        Ok(())
    } else {
        let desc = resp
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        if let Some(retry_after) = parse_retry_after(desc) {
            Err(anyhow::anyhow!(
                "Rate-limited for {}s ({:.1}h) — health-monitor will handle it",
                retry_after,
                retry_after as f64 / 3600.0
            ))
        } else {
            Err(anyhow::anyhow!("Bot API error: {}", desc))
        }
    }
}

/// Set bot display name via `setMyName`.
async fn set_bot_name(bot: &Bot, name: &str) -> anyhow::Result<()> {
    let resp: serde_json::Value = bot
        .client()
        .post(api_url(bot, "setMyName"))
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await?
        .json()
        .await?;

    if resp.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        Ok(())
    } else {
        let desc = resp
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        if let Some(retry_after) = parse_retry_after(desc) {
            Err(anyhow::anyhow!(
                "Rate-limited for {}s ({:.1}h) — health-monitor will handle it",
                retry_after,
                retry_after as f64 / 3600.0
            ))
        } else {
            Err(anyhow::anyhow!("setMyName error: {}", desc))
        }
    }
}

/// Returns `true` if the bot is running in staging mode (`DORADURA_STAGING=1`).
pub fn is_staging() -> bool {
    std::env::var("DORADURA_STAGING").is_ok_and(|v| v == "1")
}

/// Set online avatar and name.
///
/// Name is set first (lightweight, less rate-limited).
/// If avatar fails due to rate limit, name may still succeed.
/// In staging mode, sets the name to "Dora at Rehearsal" instead of the production name.
pub async fn set_online_avatar(bot: &Bot) -> anyhow::Result<()> {
    // Name first — it's what users actually see, and is less rate-limited
    let name = if is_staging() { STAGING_NAME } else { ONLINE_NAME };
    if let Err(e) = set_bot_name(bot, name).await {
        log::warn!("Failed to set online bot name: {}", e);
    }
    set_bot_avatar(bot, ONLINE_AVATAR).await?;
    Ok(())
}

/// Set offline avatar and name.
pub async fn set_offline_avatar(bot: &Bot) -> anyhow::Result<()> {
    if let Err(e) = set_bot_name(bot, OFFLINE_NAME).await {
        log::warn!("Failed to set offline bot name: {}", e);
    }
    set_bot_avatar(bot, OFFLINE_AVATAR).await?;
    Ok(())
}
