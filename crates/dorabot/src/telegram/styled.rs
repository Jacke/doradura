//! Styled inline keyboard buttons via direct Bot API HTTP calls (Bot API 9.4+).
//!
//! Teloxide (Bot API 9.1) doesn't support the `style` field on
//! `InlineKeyboardButton`. This module post-processes a standard
//! `InlineKeyboardMarkup`, injects `"style"` based on callback-data
//! patterns, and sends the result through the bot's reqwest client.

use serde_json::{json, Value};
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, MessageId, ParseMode};

use super::Bot;

// ── Style rules ──────────────────────────────────────────────────────

/// Derive a button style from its callback data.
///
/// Returns `Some("success")` for download buttons, `Some("danger")` for
/// cancel/reset buttons, or `None` for everything else.
fn style_for_callback(cb_data: &str) -> Option<&'static str> {
    if cb_data.starts_with("dl:") {
        return Some("success"); // green
    }
    if cb_data.contains("cancel") || cb_data == "ct:all:0" {
        return Some("danger"); // red
    }
    None
}

/// Convert an `InlineKeyboardMarkup` to a JSON value, injecting
/// `"style"` into every button that matches a rule.
pub fn inject_styles(keyboard: &InlineKeyboardMarkup) -> Value {
    let mut rows: Vec<Value> = Vec::new();

    for row in &keyboard.inline_keyboard {
        let mut json_row: Vec<Value> = Vec::new();
        for button in row {
            let mut btn_json = serde_json::to_value(button).unwrap_or_default();

            if let Some(cb_data) = btn_json.get("callback_data").and_then(Value::as_str) {
                if let Some(style) = style_for_callback(cb_data) {
                    btn_json.as_object_mut().unwrap().insert("style".into(), json!(style));
                }
            }

            json_row.push(btn_json);
        }
        rows.push(json!(json_row));
    }

    json!({ "inline_keyboard": rows })
}

// ── Error type ───────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum StyledError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Telegram API: {0}")]
    Api(String),
}

impl From<StyledError> for teloxide::RequestError {
    fn from(e: StyledError) -> Self {
        teloxide::RequestError::Api(teloxide::ApiError::Unknown(e.to_string()))
    }
}

// ── Telegram response parsing ────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
struct TgMessageResponse {
    ok: bool,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    result: Option<Value>,
}

fn api_url(bot: &Bot, method: &str) -> String {
    format!(
        "{}/bot{}/{}",
        bot.api_url().as_str().trim_end_matches('/'),
        bot.token(),
        method
    )
}

/// Parse `Message` from Telegram JSON response (best-effort; returns
/// `None` if deserialization fails — callers still get the side-effect).
fn parse_message(resp: &TgMessageResponse) -> Option<teloxide::types::Message> {
    resp.result
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

// ── Public send / edit helpers ───────────────────────────────────────

/// Send a text message with a styled inline keyboard.
pub async fn send_message_styled(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
    keyboard: &InlineKeyboardMarkup,
    parse_mode: Option<ParseMode>,
) -> Result<Option<teloxide::types::Message>, StyledError> {
    let mut body = json!({
        "chat_id": chat_id.0,
        "text": text,
        "reply_markup": inject_styles(keyboard),
    });
    if let Some(pm) = parse_mode {
        body["parse_mode"] = json!(format!("{:?}", pm));
    }

    let resp: TgMessageResponse = bot
        .client()
        .post(api_url(bot, "sendMessage"))
        .json(&body)
        .send()
        .await?
        .json()
        .await?;

    if resp.ok {
        Ok(parse_message(&resp))
    } else {
        Err(StyledError::Api(resp.description.unwrap_or_default()))
    }
}

/// Send a photo (raw bytes) with caption and a styled inline keyboard.
pub async fn send_photo_styled(
    bot: &Bot,
    chat_id: ChatId,
    photo_bytes: Vec<u8>,
    caption: &str,
    keyboard: &InlineKeyboardMarkup,
    parse_mode: Option<ParseMode>,
) -> Result<Option<teloxide::types::Message>, StyledError> {
    let reply_markup = serde_json::to_string(&inject_styles(keyboard)).unwrap_or_default();

    let photo_part = reqwest::multipart::Part::bytes(photo_bytes)
        .file_name("photo.jpg")
        .mime_str("image/jpeg")
        .unwrap();

    let mut form = reqwest::multipart::Form::new()
        .text("chat_id", chat_id.0.to_string())
        .text("caption", caption.to_owned())
        .text("reply_markup", reply_markup)
        .part("photo", photo_part);

    if let Some(pm) = parse_mode {
        form = form.text("parse_mode", format!("{:?}", pm));
    }

    let resp: TgMessageResponse = bot
        .client()
        .post(api_url(bot, "sendPhoto"))
        .multipart(form)
        .send()
        .await?
        .json()
        .await?;

    if resp.ok {
        Ok(parse_message(&resp))
    } else {
        Err(StyledError::Api(resp.description.unwrap_or_default()))
    }
}

/// Edit a text message with a styled inline keyboard.
pub async fn edit_message_text_styled(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    text: &str,
    keyboard: &InlineKeyboardMarkup,
    parse_mode: Option<ParseMode>,
) -> Result<(), StyledError> {
    let mut body = json!({
        "chat_id": chat_id.0,
        "message_id": message_id.0,
        "text": text,
        "reply_markup": inject_styles(keyboard),
    });
    if let Some(pm) = parse_mode {
        body["parse_mode"] = json!(format!("{:?}", pm));
    }

    let resp: TgMessageResponse = bot
        .client()
        .post(api_url(bot, "editMessageText"))
        .json(&body)
        .send()
        .await?
        .json()
        .await?;

    if resp.ok {
        Ok(())
    } else {
        Err(StyledError::Api(resp.description.unwrap_or_default()))
    }
}

/// Edit a message caption with a styled inline keyboard.
pub async fn edit_message_caption_styled(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    caption: &str,
    keyboard: &InlineKeyboardMarkup,
    parse_mode: Option<ParseMode>,
) -> Result<(), StyledError> {
    let mut body = json!({
        "chat_id": chat_id.0,
        "message_id": message_id.0,
        "caption": caption,
        "reply_markup": inject_styles(keyboard),
    });
    if let Some(pm) = parse_mode {
        body["parse_mode"] = json!(format!("{:?}", pm));
    }

    let resp: TgMessageResponse = bot
        .client()
        .post(api_url(bot, "editMessageCaption"))
        .json(&body)
        .send()
        .await?
        .json()
        .await?;

    if resp.ok {
        Ok(())
    } else {
        Err(StyledError::Api(resp.description.unwrap_or_default()))
    }
}

/// Edit ONLY the reply markup (keyboard) of a message, with styles.
pub async fn edit_message_reply_markup_styled(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    keyboard: &InlineKeyboardMarkup,
) -> Result<(), StyledError> {
    let body = json!({
        "chat_id": chat_id.0,
        "message_id": message_id.0,
        "reply_markup": inject_styles(keyboard),
    });

    let resp: TgMessageResponse = bot
        .client()
        .post(api_url(bot, "editMessageReplyMarkup"))
        .json(&body)
        .send()
        .await?
        .json()
        .await?;

    if resp.ok {
        Ok(())
    } else {
        Err(StyledError::Api(resp.description.unwrap_or_default()))
    }
}
