//! Extension trait for Telegram Bot API `sendMessageDraft` (Bot API 9.3+).
//!
//! Enables real-time message streaming: send partial text that updates
//! in the user's chat as it's being generated, then finalize with
//! `sendMessage` / `editMessageText`.
//!
//! Teloxide doesn't support this method yet (stuck on Bot API 9.1),
//! so we call the HTTP endpoint directly via the bot's reqwest client.

use serde::Serialize;
use teloxide::prelude::*;
use teloxide::types::MessageEntity;

use super::Bot;

/// Parameters for `sendMessageDraft`.
#[derive(Debug, Serialize)]
pub struct SendMessageDraftParams {
    pub chat_id: ChatId,
    pub draft_id: i64,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entities: Option<Vec<MessageEntity>>,
}

/// Response from the Telegram API.
#[derive(Debug, serde::Deserialize)]
struct TgResponse {
    ok: bool,
    #[serde(default)]
    description: Option<String>,
}

/// Error type for draft operations.
#[derive(Debug, thiserror::Error)]
pub enum DraftError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Telegram API error: {0}")]
    Api(String),
}

/// Extension trait adding `sendMessageDraft` support to the bot.
pub trait BotDraftExt {
    /// Stream a partial message to a user while it's being generated.
    ///
    /// # Arguments
    /// - `chat_id` — target chat
    /// - `draft_id` — unique draft identifier (use the same id for
    ///   consecutive updates of the same draft; Telegram replaces the
    ///   previous content each time)
    /// - `text` — current (partial) text of the message
    ///
    /// # Example
    /// ```ignore
    /// use crate::telegram::draft::BotDraftExt;
    ///
    /// let draft_id = 1;
    /// bot.send_message_draft(chat_id, draft_id, "Generating...").await?;
    /// bot.send_message_draft(chat_id, draft_id, "Generating... done!").await?;
    /// // Finalize with a real message:
    /// bot.send_message(chat_id, "Final result").await?;
    /// ```
    fn send_message_draft(
        &self,
        chat_id: ChatId,
        draft_id: i64,
        text: impl Into<String> + Send,
    ) -> impl std::future::Future<Output = Result<(), DraftError>> + Send;

    /// Same as `send_message_draft` but with full parameter control.
    fn send_message_draft_full(
        &self,
        params: SendMessageDraftParams,
    ) -> impl std::future::Future<Output = Result<(), DraftError>> + Send;
}

impl BotDraftExt for Bot {
    async fn send_message_draft(
        &self,
        chat_id: ChatId,
        draft_id: i64,
        text: impl Into<String> + Send,
    ) -> Result<(), DraftError> {
        self.send_message_draft_full(SendMessageDraftParams {
            chat_id,
            draft_id,
            text: text.into(),
            message_thread_id: None,
            parse_mode: None,
            entities: None,
        })
        .await
    }

    async fn send_message_draft_full(&self, params: SendMessageDraftParams) -> Result<(), DraftError> {
        let url = format!(
            "{}/bot{}/sendMessageDraft",
            self.api_url().as_str().trim_end_matches('/'),
            self.token()
        );

        log::debug!(
            "sendMessageDraft chat_id={} draft_id={} text_len={}",
            params.chat_id,
            params.draft_id,
            params.text.len()
        );

        let resp: TgResponse = self.client().post(&url).json(&params).send().await?.json().await?;

        if resp.ok {
            Ok(())
        } else {
            Err(DraftError::Api(
                resp.description.unwrap_or_else(|| "unknown error".into()),
            ))
        }
    }
}
