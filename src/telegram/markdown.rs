use crate::core::utils::escape_markdown_v2;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, ParseMode};
use teloxide::RequestError;

fn is_markdown_parse_error(err: &RequestError) -> bool {
    err.to_string().to_lowercase().contains("can't parse entities")
}

/// Send a MarkdownV2 message and auto-escape on parse errors.
pub async fn send_message_markdown_v2(
    bot: &Bot,
    chat_id: ChatId,
    text: impl Into<String>,
    keyboard: Option<InlineKeyboardMarkup>,
) -> ResponseResult<Message> {
    let raw_text = text.into();
    let mut req = bot
        .send_message(chat_id, raw_text.clone())
        .parse_mode(ParseMode::MarkdownV2);
    if let Some(kb) = keyboard.clone() {
        req = req.reply_markup(kb);
    }

    match req.await {
        Ok(msg) => Ok(msg),
        Err(e) if is_markdown_parse_error(&e) => {
            let escaped = escape_markdown_v2(&raw_text);
            let mut retry = bot.send_message(chat_id, escaped).parse_mode(ParseMode::MarkdownV2);
            if let Some(kb) = keyboard {
                retry = retry.reply_markup(kb);
            }
            retry.await
        }
        Err(e) => Err(e),
    }
}
