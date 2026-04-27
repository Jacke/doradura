// ── Re-export shared progress types from doracore ─────────────────────────────
pub use doracore::download::progress::{DownloadStatus, ProgressBarStyle, create_progress_bar, source_display_name};

use crate::core::extract_retry_after;
use crate::telegram::{Bot, BotExt};
use teloxide::prelude::*;
use teloxide::types::MessageId;
use unic_langid::LanguageIdentifier;

// ── Bot-specific: Telegram progress message management ───────────────────────

/// Structure for managing the download progress message.
///
/// Tracks the progress message ID and allows updating it as the download proceeds.
pub struct ProgressMessage {
    /// User's chat ID
    pub chat_id: ChatId,
    /// Progress message ID (None if not yet sent)
    pub message_id: Option<MessageId>,
    /// User's language for localizing progress messages
    pub lang: LanguageIdentifier,
    /// Progress bar style chosen by the user
    pub style: ProgressBarStyle,
    /// Source and quality badge (e.g. "YouTube · MP3 320kbps")
    pub source_badge: Option<String>,
    /// Last successful editMessageText time — used for throttling to avoid Telegram 429
    last_edit_at: Option<std::time::Instant>,
}

impl ProgressMessage {
    /// Creates a new progress message for the specified chat.
    pub fn new(chat_id: ChatId, lang: LanguageIdentifier) -> Self {
        Self {
            chat_id,
            message_id: None,
            lang,
            style: ProgressBarStyle::default(),
            source_badge: None,
            last_edit_at: None,
        }
    }

    /// Minimum interval between editMessageText calls (prevents Telegram 429).
    const EDIT_THROTTLE: std::time::Duration = std::time::Duration::from_millis(1000);

    /// Creates a shallow copy suitable for `clear_after_delay` (shares message_id).
    pub fn clone_for_clear(&self) -> Self {
        Self {
            chat_id: self.chat_id,
            message_id: self.message_id,
            lang: self.lang.clone(),
            style: self.style,
            source_badge: self.source_badge.clone(),
            last_edit_at: None,
        }
    }

    /// Build the inline keyboard attached to in-progress messages. Currently
    /// just a single "❌ Cancel" button (GH #9). Returns `None` for terminal
    /// statuses (success / error / cancellation) so the keyboard is removed
    /// at the end of the lifecycle.
    fn keyboard_for(&self, status: &DownloadStatus) -> Option<teloxide::types::InlineKeyboardMarkup> {
        use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};
        let show = matches!(
            status,
            DownloadStatus::Starting { .. } | DownloadStatus::Downloading { .. } | DownloadStatus::Merging { .. }
        );
        if !show {
            return None;
        }
        let label = crate::i18n::t(&self.lang, "download_cancel.button");
        Some(InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
            label,
            "dl_cancel:1",
        )]]))
    }

    pub async fn update(&mut self, bot: &Bot, status: DownloadStatus) -> ResponseResult<()> {
        let text = status.to_message(&self.lang, self.style, self.source_badge.as_deref());
        let keyboard = self.keyboard_for(&status);

        if let Some(msg_id) = self.message_id {
            // Throttle edits: skip if last successful edit was < 1.5s ago
            // (except for non-Downloading statuses which are important transitions)
            if matches!(status, DownloadStatus::Downloading { .. })
                && let Some(last) = self.last_edit_at
                && last.elapsed() < Self::EDIT_THROTTLE
            {
                return Ok(());
            }

            // Update existing message
            let mut req = bot
                .edit_message_text(self.chat_id, msg_id, text.clone())
                .parse_mode(teloxide::types::ParseMode::MarkdownV2);
            if let Some(kbd) = keyboard.clone() {
                req = req.reply_markup(kbd);
            }
            match req.await {
                Ok(_) => {
                    self.last_edit_at = Some(std::time::Instant::now());
                    Ok(())
                }
                Err(e) => {
                    let error_str = e.to_string();
                    // If the message hasn't changed - that's fine, no need to send a new one
                    if error_str.contains("message is not modified") {
                        return Ok(());
                    }

                    // Check rate limiting
                    if let Some(retry_after_secs) = extract_retry_after(&error_str) {
                        log::warn!(
                            "Rate limit hit when editing message: Retry after {}s. Waiting...",
                            retry_after_secs
                        );
                        tokio::time::sleep(tokio::time::Duration::from_secs(retry_after_secs + 1)).await;
                        // Try to edit again
                        let mut retry_req = bot
                            .edit_message_text(self.chat_id, msg_id, text.clone())
                            .parse_mode(teloxide::types::ParseMode::MarkdownV2);
                        if let Some(kbd) = keyboard.clone() {
                            retry_req = retry_req.reply_markup(kbd);
                        }
                        match retry_req.await {
                            Ok(_) => return Ok(()),
                            Err(e2) => {
                                let error_str2 = e2.to_string();
                                if error_str2.contains("message is not modified") {
                                    return Ok(());
                                }
                                log::warn!(
                                    "Still failed to edit message after rate limit wait: {}. Trying to send new one.",
                                    e2
                                );
                            }
                        }
                    } else {
                        log::warn!("Failed to edit message: {}. Trying to send new one.", e);
                    }

                    // If editing failed for another reason, send a new message
                    let mut send_req = bot
                        .send_message(self.chat_id, text)
                        .parse_mode(teloxide::types::ParseMode::MarkdownV2);
                    if let Some(kbd) = keyboard.clone() {
                        send_req = send_req.reply_markup(kbd);
                    }
                    let msg = send_req.await?;
                    self.message_id = Some(msg.id);
                    Ok(())
                }
            }
        } else {
            // Send a new message
            let mut send_req = bot
                .send_message(self.chat_id, text)
                .parse_mode(teloxide::types::ParseMode::MarkdownV2);
            if let Some(kbd) = keyboard {
                send_req = send_req.reply_markup(kbd);
            }
            let msg = send_req.await?;
            self.message_id = Some(msg.id);
            Ok(())
        }
    }

    /// Clears the message after the specified delay by deleting it.
    pub async fn clear_after(
        &mut self,
        bot: &Bot,
        delay_secs: u64,
        _title: String,
        _file_format: Option<String>,
    ) -> ResponseResult<()> {
        if let Some(msg_id) = self.message_id.take() {
            tokio::time::sleep(tokio::time::Duration::from_secs(delay_secs)).await;
            bot.try_delete(self.chat_id, msg_id).await;
            log::info!(
                "Deleted progress message for chat {} after {} seconds",
                self.chat_id,
                delay_secs
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::escape_markdown_v2 as escape_markdown;

    fn test_lang() -> LanguageIdentifier {
        crate::i18n::lang_from_code("ru")
    }

    // ── Progress bar tests (exercising doracore re-exports) ──

    #[test]
    fn test_progress_bar() {
        let s = ProgressBarStyle::Classic;
        assert_eq!(create_progress_bar(0, s), "[░░░░░░░░░░]");
        assert_eq!(create_progress_bar(50, s), "[█████░░░░░]");
        assert_eq!(create_progress_bar(100, s), "[██████████]");
    }

    #[test]
    fn test_progress_bar_style_roundtrip() {
        for style in ProgressBarStyle::all() {
            let s = style.as_str();
            let parsed = ProgressBarStyle::parse(s);
            assert_eq!(*style, parsed, "Roundtrip failed for {}", s);
        }
    }

    #[test]
    fn test_source_display_name() {
        let yt = url::Url::parse("https://www.youtube.com/watch?v=abc").unwrap();
        assert_eq!(source_display_name(&yt), "YouTube");

        let sc = url::Url::parse("https://soundcloud.com/artist/track").unwrap();
        assert_eq!(source_display_name(&sc), "SoundCloud");

        let other = url::Url::parse("https://example.com/file.mp3").unwrap();
        assert_eq!(source_display_name(&other), "Web");
    }

    // ── escape_markdown smoke tests ──

    #[test]
    fn test_escape_markdown() {
        assert_eq!(escape_markdown("Hello World"), "Hello World");
        assert_eq!(escape_markdown("Test_file.mp3"), "Test\\_file\\.mp3");
    }

    // ── extract_retry_after ──

    #[test]
    fn test_extract_retry_after_standard() {
        assert_eq!(extract_retry_after("Retry after 30s"), Some(30));
    }

    // ── DownloadStatus integration ──

    #[test]
    fn test_status_starting_message() {
        let lang = test_lang();
        let status = DownloadStatus::Starting {
            title: "Test Song".to_string(),
            file_format: Some("mp3".to_string()),
            artist: None,
        };
        let msg = status.to_message(&lang, ProgressBarStyle::default(), None);
        assert!(msg.contains("Test Song"));
        assert!(msg.contains("⏳"));
    }

    #[test]
    fn test_status_downloading_message() {
        let lang = test_lang();
        let status = DownloadStatus::Downloading {
            title: "Test Song".to_string(),
            progress: 50,
            speed_mbs: Some(5.5),
            eta_seconds: Some(30),
            current_size: Some(50 * 1024 * 1024),
            total_size: Some(100 * 1024 * 1024),
            file_format: Some("mp3".to_string()),
            update_count: 0,
            artist: None,
        };
        let msg = status.to_message(&lang, ProgressBarStyle::default(), None);
        assert!(msg.contains("Test Song"));
        assert!(msg.contains("50%"));
    }

    #[test]
    fn test_status_message_english() {
        let lang = crate::i18n::lang_from_code("en");
        let status = DownloadStatus::Starting {
            title: "Test Song".to_string(),
            file_format: Some("mp3".to_string()),
            artist: None,
        };
        let msg = status.to_message(&lang, ProgressBarStyle::default(), None);
        assert!(msg.contains("Starting download"));
    }

    // ── ProgressMessage ──

    #[test]
    fn test_progress_message_new() {
        let lang = test_lang();
        let pm = ProgressMessage::new(ChatId(12345), lang);
        assert_eq!(pm.chat_id, ChatId(12345));
        assert!(pm.message_id.is_none());
    }
}
