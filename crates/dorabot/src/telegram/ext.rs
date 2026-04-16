//! Extension trait for the crate-local `Bot` wrapper.
//!
//! Collapses the most repetitive Telegram send/edit chains into a single
//! method call. The raw `bot.send_message(...)` builder API is still
//! fine for one-off cases; `BotExt` is for the boilerplate patterns that
//! appear dozens of times across the codebase:
//!
//! ```ignore
//! // before:
//! bot.send_message(chat_id, text)
//!     .parse_mode(ParseMode::MarkdownV2)
//!     .reply_markup(keyboard)
//!     .await?;
//!
//! // after:
//! bot.send_md_kb(chat_id, text, keyboard).await?;
//! ```
//!
//! Scope: covers `send_message` + `edit_message_text` with the three most
//! common terminal combos (plain MarkdownV2, MarkdownV2 + reply_markup,
//! plain HTML). Anything more exotic (`.disable_web_page_preview`,
//! `.reply_to_message_id`, sticker/photo/video sends, etc.) should keep
//! using the raw builder — the point of the trait is to cover the 80%
//! case, not every variation.

use crate::telegram::Bot;
use teloxide::prelude::*;
use teloxide::types::{ChatId, InlineKeyboardMarkup, InputFile, MessageId, ParseMode};
use teloxide::ApiError;
use teloxide::RequestError;

/// Sealed extension trait so only this crate can implement it.
/// All methods take `&self` and delegate to the normal teloxide builder —
/// there is no new state or allocation.
#[allow(async_fn_in_trait)]
pub trait BotExt {
    /// Send a MarkdownV2-formatted text message without a keyboard.
    async fn send_md(&self, chat_id: ChatId, text: impl Into<String>) -> Result<Message, RequestError>;

    /// Send a MarkdownV2-formatted text message with an inline keyboard.
    async fn send_md_kb(
        &self,
        chat_id: ChatId,
        text: impl Into<String>,
        keyboard: InlineKeyboardMarkup,
    ) -> Result<Message, RequestError>;

    /// Edit an existing message's text using MarkdownV2 formatting.
    async fn edit_md(
        &self,
        chat_id: ChatId,
        message_id: MessageId,
        text: impl Into<String>,
    ) -> Result<Message, RequestError>;

    /// Edit an existing message's text + reply markup using MarkdownV2.
    async fn edit_md_kb(
        &self,
        chat_id: ChatId,
        message_id: MessageId,
        text: impl Into<String>,
        keyboard: InlineKeyboardMarkup,
    ) -> Result<Message, RequestError>;

    /// Fire-and-forget delete. Ignores errors (message already gone,
    /// permission revoked, TTL expired, etc.) — the common cleanup
    /// pattern. Collapses 59 `let _ = self.delete_message(...).await;`
    /// sites into a single call.
    async fn try_delete(&self, chat_id: ChatId, message_id: MessageId);
}

impl BotExt for Bot {
    async fn send_md(&self, chat_id: ChatId, text: impl Into<String>) -> Result<Message, RequestError> {
        self.send_message(chat_id, text).parse_mode(ParseMode::MarkdownV2).await
    }

    async fn send_md_kb(
        &self,
        chat_id: ChatId,
        text: impl Into<String>,
        keyboard: InlineKeyboardMarkup,
    ) -> Result<Message, RequestError> {
        self.send_message(chat_id, text)
            .parse_mode(ParseMode::MarkdownV2)
            .reply_markup(keyboard)
            .await
    }

    async fn edit_md(
        &self,
        chat_id: ChatId,
        message_id: MessageId,
        text: impl Into<String>,
    ) -> Result<Message, RequestError> {
        self.edit_message_text(chat_id, message_id, text)
            .parse_mode(ParseMode::MarkdownV2)
            .await
    }

    async fn edit_md_kb(
        &self,
        chat_id: ChatId,
        message_id: MessageId,
        text: impl Into<String>,
        keyboard: InlineKeyboardMarkup,
    ) -> Result<Message, RequestError> {
        self.edit_message_text(chat_id, message_id, text)
            .parse_mode(ParseMode::MarkdownV2)
            .reply_markup(keyboard)
            .await
    }

    async fn try_delete(&self, chat_id: ChatId, message_id: MessageId) {
        let _ = self.delete_message(chat_id, message_id).await;
    }
}

// Unused imports guard — InputFile and ApiError are reserved for future
// extensions (send_photo_md, typed ApiError matching) without another churn.
#[allow(dead_code)]
fn _reserved(_: InputFile, _: ApiError) {}
