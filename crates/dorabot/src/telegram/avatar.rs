//! Bot avatar & name status indicator.
//!
//! Changes the bot's profile photo and display name to reflect online/offline status.
//! Uses raw Bot API calls since teloxide doesn't support `setMyProfilePhoto` / `setMyName`.

use super::Bot;

const ONLINE_AVATAR: &[u8] = include_bytes!("../../../../assets/avatar/online.png");
const OFFLINE_AVATAR: &[u8] = include_bytes!("../../../../assets/avatar/offline.png");

const ONLINE_NAME: &str = "Dora – Downloader Youtube Instagram TikTok";
const OFFLINE_NAME: &str = "Dora – Sleep";

/// Always use official Telegram API for profile methods — Local Bot API
/// often doesn't support newer methods like setMyProfilePhoto (Bot API 8.2+).
const TELEGRAM_API: &str = "https://api.telegram.org";

fn api_url(bot: &Bot, method: &str) -> String {
    format!("{}/bot{}/{}", TELEGRAM_API, bot.token(), method)
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
        Err(anyhow::anyhow!("Bot API error: {}", desc))
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
        Err(anyhow::anyhow!("setMyName error: {}", desc))
    }
}

/// Set online avatar and name.
pub async fn set_online_avatar(bot: &Bot) -> anyhow::Result<()> {
    set_bot_avatar(bot, ONLINE_AVATAR).await?;
    if let Err(e) = set_bot_name(bot, ONLINE_NAME).await {
        log::warn!("Failed to set online bot name: {}", e);
    }
    Ok(())
}

/// Set offline avatar and name.
pub async fn set_offline_avatar(bot: &Bot) -> anyhow::Result<()> {
    set_bot_avatar(bot, OFFLINE_AVATAR).await?;
    if let Err(e) = set_bot_name(bot, OFFLINE_NAME).await {
        log::warn!("Failed to set offline bot name: {}", e);
    }
    Ok(())
}
