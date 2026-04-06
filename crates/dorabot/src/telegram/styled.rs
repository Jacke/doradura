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

/// Prefix → style mapping table. Checked in order; first match wins.
const STYLE_RULES: &[(&str, &str)] = &[
    // Green (success) — send / resend / download
    ("dl:", "success"),
    ("downloads:resend:", "success"),
    ("downloads:resend_cut:", "success"),
    ("downloads:send:", "success"),
    ("downloads:send_cut:", "success"),
    // Red (danger) — close
    ("downloads:close", "danger"),
    // Blue (primary) — filters / actions
    ("downloads:filter:", "primary"),
    ("downloads:catfilter:", "primary"),
    ("downloads:clip:", "primary"),
    ("downloads:clip_cut:", "primary"),
    ("downloads:circle:", "primary"),
    ("downloads:circle_cut:", "primary"),
    ("downloads:speed:", "primary"),
    ("downloads:speed_cut:", "primary"),
    ("downloads:setcat:", "primary"),
    ("downloads:dur:", "primary"),
    ("ringtone:select:", "primary"),
];

/// Derive a button style from its callback data.
fn style_for_callback(cb_data: &str) -> Option<&'static str> {
    // Substring match for cancel variants (cancel, clip_cancel, etc.)
    if cb_data.contains("cancel") {
        return Some("danger");
    }
    STYLE_RULES
        .iter()
        .find(|(prefix, _)| cb_data.starts_with(prefix))
        .map(|(_, style)| *style)
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
                    if let Some(obj) = btn_json.as_object_mut() {
                        obj.insert("style".into(), json!(style));
                    }
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
    send_message_styled_inner(bot, chat_id, text, keyboard, parse_mode, false).await
}

/// Inner implementation with optional link preview control.
async fn send_message_styled_inner(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
    keyboard: &InlineKeyboardMarkup,
    parse_mode: Option<ParseMode>,
    disable_link_preview: bool,
) -> Result<Option<teloxide::types::Message>, StyledError> {
    let mut body = json!({
        "chat_id": chat_id.0,
        "text": text,
        "reply_markup": inject_styles(keyboard),
    });
    if let Some(pm) = parse_mode {
        body["parse_mode"] = json!(format!("{:?}", pm));
    }
    if disable_link_preview {
        body["link_preview_options"] = json!({"is_disabled": true});
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

// ── Convenience: styled send with automatic fallback ────────────────

/// Send a text message with styled buttons, falling back to standard
/// teloxide on HTTP/API error. Does NOT double-send when styled
/// succeeds but response parsing fails (`Ok(None)`).
pub async fn send_message_styled_or_fallback(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
    keyboard: &InlineKeyboardMarkup,
    parse_mode: Option<ParseMode>,
) -> ResponseResult<teloxide::types::Message> {
    send_message_styled_or_fallback_opts(bot, chat_id, text, keyboard, parse_mode, false).await
}

/// Like [`send_message_styled_or_fallback`] but with link-preview control.
pub async fn send_message_styled_or_fallback_opts(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
    keyboard: &InlineKeyboardMarkup,
    parse_mode: Option<ParseMode>,
    disable_link_preview: bool,
) -> ResponseResult<teloxide::types::Message> {
    match send_message_styled_inner(bot, chat_id, text, keyboard, parse_mode, disable_link_preview).await {
        Ok(Some(msg)) => Ok(msg),
        Err(e) => {
            log::debug!("Styled send failed ({}), falling back to teloxide", e);
            let mut req = bot.send_message(chat_id, text).reply_markup(keyboard.clone());
            if let Some(pm) = parse_mode {
                req = req.parse_mode(pm);
            }
            if disable_link_preview {
                req = req.link_preview_options(teloxide::types::LinkPreviewOptions {
                    is_disabled: true,
                    url: None,
                    prefer_small_media: false,
                    prefer_large_media: false,
                    show_above_text: false,
                });
            }
            req.await
        }
        // Ok(None): Telegram accepted the message (user sees it) but we
        // couldn't parse the response. Fetch a minimal stand-in rather
        // than sending a duplicate.
        Ok(None) => {
            log::warn!("Styled send ok but response parse failed; not re-sending");
            Err(teloxide::RequestError::Api(teloxide::ApiError::Unknown(
                "styled response parse failed".into(),
            )))
        }
    }
}
