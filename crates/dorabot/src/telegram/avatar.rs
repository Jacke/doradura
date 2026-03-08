//! Bot avatar status indicator.
//!
//! Changes the bot's profile photo to reflect online/offline status.
//! Uses raw Bot API calls since teloxide doesn't support `setMyProfilePhoto`.

use super::Bot;

const ONLINE_AVATAR: &[u8] = include_bytes!("../../../../assets/avatar/online.png");
const OFFLINE_AVATAR: &[u8] = include_bytes!("../../../../assets/avatar/offline.png");

fn api_url(bot: &Bot, method: &str) -> String {
    format!(
        "{}/bot{}/{}",
        bot.api_url().as_str().trim_end_matches('/'),
        bot.token(),
        method
    )
}

/// Set bot profile photo from PNG bytes.
async fn set_bot_avatar(bot: &Bot, photo_bytes: &[u8]) -> anyhow::Result<()> {
    let photo_part = reqwest::multipart::Part::bytes(photo_bytes.to_vec())
        .file_name("photo.png")
        .mime_str("image/png")
        .unwrap();

    let form = reqwest::multipart::Form::new().part("photo", photo_part);

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

/// Set online avatar (normal profile photo).
pub async fn set_online_avatar(bot: &Bot) -> anyhow::Result<()> {
    set_bot_avatar(bot, ONLINE_AVATAR).await
}

/// Set offline avatar (crossed-out profile photo).
pub async fn set_offline_avatar(bot: &Bot) -> anyhow::Result<()> {
    set_bot_avatar(bot, OFFLINE_AVATAR).await
}
