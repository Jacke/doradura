//! Unified interface for operations with progress tracking.
//!
//! Provides a reusable abstraction for any operations requiring:
//! - Reaction on the user's message at start
//! - Progress bar display (with updates)
//! - Progress bar removal and result display on completion
//! - Reaction on the message on success/error
//!
//! # Typestate Pattern
//!
//! This module uses the typestate pattern to enforce correct usage at compile time.
//! Operations must progress through states: `NotStarted` → `InProgress` → `Completed`.
//!
//! # Example
//!
//! ```ignore
//! use doradura::telegram::operation::{Operation, OperationBuilder};
//! use teloxide::types::ChatId;
//!
//! // Build operation with builder pattern
//! let op = Operation::builder(bot, ChatId(123))
//!     .user_message(msg_id)
//!     .title("Creating video note")
//!     .emoji("🎥")
//!     .build();
//!
//! // Start operation (transitions NotStarted → InProgress)
//! let mut op = op.start().await?;
//!
//! // Update progress (only available in InProgress state)
//! op.update_progress(50, Some("Processing video")).await?;
//!
//! // Complete (transitions InProgress → Completed)
//! let op = op.complete_success(Some("Video note ready!"), 5).await?;
//! ```

use crate::core::utils::escape_markdown_v2;
use crate::telegram::reactions::{emoji, try_set_reaction};
use crate::telegram::Bot;
use std::marker::PhantomData;
use std::time::{Duration, Instant};
use teloxide::prelude::*;
use teloxide::types::{MessageId, ParseMode};
use thiserror::Error;
use tokio::task::JoinHandle;

// ============================================================================
// Constants
// ============================================================================

/// Default emoji shown when no custom emoji is provided.
pub const DEFAULT_EMOJI: &str = "⚙️";

/// Minimum interval between message updates to avoid rate limiting.
const MIN_UPDATE_INTERVAL: Duration = Duration::from_millis(500);

// ============================================================================
// Error Types (using thiserror)
// ============================================================================

/// Operation error types.
#[derive(Debug, Error)]
pub enum OperationError {
    /// Telegram API error
    #[error("Telegram error: {0}")]
    Telegram(#[from] teloxide::RequestError),

    /// Generic error (for anyhow compatibility)
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

// ============================================================================
// Typestate Markers
// ============================================================================

/// Marker type: Operation has not been started yet.
#[derive(Debug, Clone, Copy)]
pub struct NotStarted;

/// Marker type: Operation is in progress.
#[derive(Debug, Clone, Copy)]
pub struct InProgress;

/// Marker type: Operation has completed (success or error).
#[derive(Debug, Clone, Copy)]
pub struct Completed;

// ============================================================================
// Operation Info
// ============================================================================

/// Common operation metadata (title and emoji).
#[derive(Debug, Clone)]
pub struct OperationInfo {
    /// Operation name/description
    pub title: String,
    /// Emoji for operation type
    pub emoji: String,
}

impl OperationInfo {
    /// Creates new operation info with optional emoji.
    pub fn new(title: impl Into<String>, emoji: Option<&str>) -> Self {
        Self {
            title: title.into(),
            emoji: emoji.unwrap_or(DEFAULT_EMOJI).to_string(),
        }
    }
}

impl Default for OperationInfo {
    fn default() -> Self {
        Self {
            title: "Operation".to_string(),
            emoji: DEFAULT_EMOJI.to_string(),
        }
    }
}

// ============================================================================
// Operation Status
// ============================================================================

/// Operation status for display to the user.
#[derive(Debug, Clone)]
pub enum OperationStatus {
    /// Operation starting
    Starting(OperationInfo),

    /// Operation in progress with percentage
    Progress {
        info: OperationInfo,
        progress: u8,
        stage: Option<String>,
    },

    /// Operation working (indeterminate progress)
    Working {
        info: OperationInfo,
        stage: Option<String>,
        dots: u8,
    },

    /// Successful completion
    Success {
        info: OperationInfo,
        message: Option<String>,
    },

    /// Error
    Error { info: OperationInfo, error: String },
}

impl OperationStatus {
    /// Returns reference to operation info.
    #[must_use]
    pub fn info(&self) -> &OperationInfo {
        match self {
            Self::Starting(info)
            | Self::Progress { info, .. }
            | Self::Working { info, .. }
            | Self::Success { info, .. }
            | Self::Error { info, .. } => info,
        }
    }

    /// Returns mutable reference to operation info.
    pub fn info_mut(&mut self) -> &mut OperationInfo {
        match self {
            Self::Starting(info)
            | Self::Progress { info, .. }
            | Self::Working { info, .. }
            | Self::Success { info, .. }
            | Self::Error { info, .. } => info,
        }
    }

    /// Returns true if this is a terminal state (Success or Error).
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Success { .. } | Self::Error { .. })
    }
}

// ============================================================================
// Message Formatter Trait
// ============================================================================

/// Trait for formatting operation status messages.
pub trait MessageFormatter: Send + Sync {
    /// Formats the status for display.
    fn format(&self, status: &OperationStatus) -> String;

    /// Formats the final message (title only).
    fn format_final(&self, status: &OperationStatus) -> String;
}

/// MarkdownV2 formatter for Telegram messages.
#[derive(Debug, Clone, Copy, Default)]
pub struct MarkdownV2Formatter;

impl MessageFormatter for MarkdownV2Formatter {
    fn format(&self, status: &OperationStatus) -> String {
        match status {
            OperationStatus::Starting(info) => {
                let escaped = escape_markdown_v2(&info.title);
                format!("{} *{}*\n\n⏳ Starting\\.\\.\\.", info.emoji, escaped)
            }
            OperationStatus::Progress { info, progress, stage } => {
                let escaped = escape_markdown_v2(&info.title);
                let bar = create_progress_bar(*progress);
                let mut msg = format!("{} *{}*\n\n📊 Progress: {}%\n{}", info.emoji, escaped, progress, bar);
                if let Some(stage_text) = stage {
                    msg.push_str("\n\n");
                    msg.push_str(&escape_markdown_v2(stage_text));
                }
                msg
            }
            OperationStatus::Working { info, stage, dots } => {
                let escaped = escape_markdown_v2(&info.title);
                let dots_str = match dots % 4 {
                    1 => "\\.",
                    2 => "\\.\\.",
                    3 => "\\.\\.\\.",
                    _ => "",
                };
                let mut msg = format!("{} *{}*\n\n⏳ Working{}", info.emoji, escaped, dots_str);
                if let Some(stage_text) = stage {
                    msg.push_str("\n\n");
                    msg.push_str(&escape_markdown_v2(stage_text));
                }
                msg
            }
            OperationStatus::Success { info, message } => {
                let escaped = escape_markdown_v2(&info.title);
                let mut msg = format!("{} *{}*\n\n✅ Done\\!", info.emoji, escaped);
                if let Some(success_msg) = message {
                    msg.push('\n');
                    msg.push_str(&escape_markdown_v2(success_msg));
                }
                msg
            }
            OperationStatus::Error { info, error } => {
                let escaped_title = escape_markdown_v2(&info.title);
                let escaped_error = escape_markdown_v2(error);
                format!("{} *{}*\n\n❌ Error: {}", info.emoji, escaped_title, escaped_error)
            }
        }
    }

    fn format_final(&self, status: &OperationStatus) -> String {
        let info = status.info();
        let escaped = escape_markdown_v2(&info.title);
        format!("{} *{}*", info.emoji, escaped)
    }
}

/// Plain text formatter (for logging/testing).
#[derive(Debug, Clone, Copy, Default)]
pub struct PlainTextFormatter;

impl MessageFormatter for PlainTextFormatter {
    fn format(&self, status: &OperationStatus) -> String {
        match status {
            OperationStatus::Starting(info) => {
                format!("{} {} - Starting...", info.emoji, info.title)
            }
            OperationStatus::Progress { info, progress, stage } => {
                let mut msg = format!("{} {} - Progress: {}%", info.emoji, info.title, progress);
                if let Some(stage_text) = stage {
                    msg.push_str(&format!(" ({})", stage_text));
                }
                msg
            }
            OperationStatus::Working { info, stage, dots } => {
                let dots_str = ".".repeat((*dots % 4) as usize);
                let mut msg = format!("{} {} - Working{}", info.emoji, info.title, dots_str);
                if let Some(stage_text) = stage {
                    msg.push_str(&format!(" ({})", stage_text));
                }
                msg
            }
            OperationStatus::Success { info, message } => {
                let mut msg = format!("{} {} - Done!", info.emoji, info.title);
                if let Some(success_msg) = message {
                    msg.push_str(&format!(" {}", success_msg));
                }
                msg
            }
            OperationStatus::Error { info, error } => {
                format!("{} {} - Error: {}", info.emoji, info.title, error)
            }
        }
    }

    fn format_final(&self, status: &OperationStatus) -> String {
        let info = status.info();
        format!("{} {}", info.emoji, info.title)
    }
}

// ============================================================================
// Operation Inner State
// ============================================================================

/// Internal operation state (shared across typestate variants).
struct OperationInner {
    bot: Bot,
    chat_id: ChatId,
    user_message_id: Option<MessageId>,
    progress_message_id: Option<MessageId>,
    current_status: Option<OperationStatus>,
    last_update: Option<Instant>,
    dots_counter: u8,
    clear_task: Option<JoinHandle<()>>,
    info: OperationInfo,
    throttle_interval: Duration,
}

impl OperationInner {
    fn cancel_clear_task(&mut self) {
        if let Some(handle) = self.clear_task.take() {
            handle.abort();
        }
    }
}

impl Drop for OperationInner {
    fn drop(&mut self) {
        self.cancel_clear_task();
    }
}

// ============================================================================
// Operation Builder
// ============================================================================

/// Builder for creating Operation instances.
#[derive(Debug)]
pub struct OperationBuilder {
    bot: Bot,
    chat_id: ChatId,
    user_message_id: Option<MessageId>,
    title: String,
    emoji: String,
    throttle_interval: Duration,
}

impl OperationBuilder {
    /// Creates a new builder with required parameters.
    pub fn new(bot: Bot, chat_id: ChatId) -> Self {
        Self {
            bot,
            chat_id,
            user_message_id: None,
            title: "Operation".to_string(),
            emoji: DEFAULT_EMOJI.to_string(),
            throttle_interval: MIN_UPDATE_INTERVAL,
        }
    }

    /// Sets the user message ID (for reactions).
    #[must_use]
    pub fn user_message(mut self, msg_id: MessageId) -> Self {
        self.user_message_id = Some(msg_id);
        self
    }

    /// Sets the operation title.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Sets the operation emoji.
    #[must_use]
    pub fn emoji(mut self, emoji: impl Into<String>) -> Self {
        self.emoji = emoji.into();
        self
    }

    /// Sets the throttle interval for updates.
    #[must_use]
    pub fn throttle_interval(mut self, interval: Duration) -> Self {
        self.throttle_interval = interval;
        self
    }

    /// Builds the Operation in NotStarted state.
    #[must_use]
    pub fn build(self) -> Operation<NotStarted> {
        Operation {
            inner: OperationInner {
                bot: self.bot,
                chat_id: self.chat_id,
                user_message_id: self.user_message_id,
                progress_message_id: None,
                current_status: None,
                last_update: None,
                dots_counter: 0,
                clear_task: None,
                info: OperationInfo {
                    title: self.title,
                    emoji: self.emoji,
                },
                throttle_interval: self.throttle_interval,
            },
            _state: PhantomData,
        }
    }
}

// ============================================================================
// Operation (Typestate)
// ============================================================================

/// Manages an operation with progress and reactions.
///
/// Uses the typestate pattern to enforce correct usage:
/// - `Operation<NotStarted>` - Can only call `start()`
/// - `Operation<InProgress>` - Can call `update_progress()`, `complete_success()`, `complete_error()`
/// - `Operation<Completed>` - Terminal state, no further operations
pub struct Operation<State = NotStarted> {
    inner: OperationInner,
    _state: PhantomData<State>,
}

// --- NotStarted State ---

impl Operation<NotStarted> {
    /// Creates a new operation builder.
    #[must_use]
    pub fn builder(bot: Bot, chat_id: ChatId) -> OperationBuilder {
        OperationBuilder::new(bot, chat_id)
    }

    /// Creates a new operation with default settings.
    ///
    /// For more control, use `Operation::builder()`.
    #[must_use]
    pub fn new(bot: Bot, chat_id: ChatId, user_message_id: Option<MessageId>) -> Self {
        let mut builder = OperationBuilder::new(bot, chat_id);
        if let Some(msg_id) = user_message_id {
            builder = builder.user_message(msg_id);
        }
        builder.build()
    }

    /// Starts the operation: sets 👀 reaction and sends "Starting..." message.
    ///
    /// Transitions from `NotStarted` to `InProgress` state.
    pub async fn start(mut self) -> anyhow::Result<Operation<InProgress>> {
        // Set reaction on user's message
        if let Some(msg_id) = self.inner.user_message_id {
            try_set_reaction(&self.inner.bot, self.inner.chat_id, msg_id, emoji::EYES).await;
        }

        // Send starting message
        let status = OperationStatus::Starting(self.inner.info.clone());
        let formatter = MarkdownV2Formatter;
        let text = formatter.format(&status);

        let msg = self
            .inner
            .bot
            .send_message(self.inner.chat_id, text)
            .parse_mode(ParseMode::MarkdownV2)
            .await?;

        self.inner.progress_message_id = Some(msg.id);
        self.inner.current_status = Some(status);
        self.inner.last_update = Some(Instant::now());

        Ok(Operation {
            inner: self.inner,
            _state: PhantomData,
        })
    }

    /// Returns the chat ID.
    #[must_use]
    pub fn chat_id(&self) -> ChatId {
        self.inner.chat_id
    }

    /// Returns the operation info.
    #[must_use]
    pub fn info(&self) -> &OperationInfo {
        &self.inner.info
    }
}

// --- InProgress State ---

impl Operation<InProgress> {
    /// Updates operation progress.
    ///
    /// Returns `Ok(true)` if message was updated, `Ok(false)` if throttled.
    #[must_use = "returns whether the update was applied"]
    pub async fn update_progress(&mut self, progress: u8, stage: Option<&str>) -> anyhow::Result<bool> {
        // Throttle updates
        if !self.should_update() {
            return Ok(false);
        }

        let status = OperationStatus::Progress {
            info: self.inner.info.clone(),
            progress: progress.min(100),
            stage: stage.map(String::from),
        };

        self.update_message(&status).await?;
        self.inner.current_status = Some(status);
        self.inner.last_update = Some(Instant::now());
        Ok(true)
    }

    /// Updates operation status without specific progress (dots animation).
    ///
    /// Returns `Ok(true)` if message was updated, `Ok(false)` if throttled.
    #[must_use = "returns whether the update was applied"]
    pub async fn update_working(&mut self, stage: Option<&str>) -> anyhow::Result<bool> {
        // Throttle updates
        if !self.should_update() {
            return Ok(false);
        }

        self.inner.dots_counter = self.inner.dots_counter.wrapping_add(1);

        let status = OperationStatus::Working {
            info: self.inner.info.clone(),
            stage: stage.map(String::from),
            dots: self.inner.dots_counter,
        };

        self.update_message(&status).await?;
        self.inner.current_status = Some(status);
        self.inner.last_update = Some(Instant::now());
        Ok(true)
    }

    /// Updates the operation title and refreshes the message.
    pub async fn set_title(&mut self, title: impl Into<String>) -> anyhow::Result<()> {
        self.inner.info.title = title.into();
        if let Some(status) = &self.inner.current_status {
            let mut new_status = status.clone();
            new_status.info_mut().title = self.inner.info.title.clone();
            self.update_message(&new_status).await?;
            self.inner.current_status = Some(new_status);
        }
        Ok(())
    }

    /// Updates the operation emoji and refreshes the message.
    pub async fn set_emoji(&mut self, emoji: impl Into<String>) -> anyhow::Result<()> {
        self.inner.info.emoji = emoji.into();
        if let Some(status) = &self.inner.current_status {
            let mut new_status = status.clone();
            new_status.info_mut().emoji = self.inner.info.emoji.clone();
            self.update_message(&new_status).await?;
            self.inner.current_status = Some(new_status);
        }
        Ok(())
    }

    /// Completes operation successfully.
    ///
    /// Transitions from `InProgress` to `Completed` state.
    pub async fn complete_success(
        mut self,
        message: Option<&str>,
        clear_after_secs: u64,
    ) -> anyhow::Result<Operation<Completed>> {
        self.inner.cancel_clear_task();

        // Set success reaction
        if let Some(msg_id) = self.inner.user_message_id {
            try_set_reaction(&self.inner.bot, self.inner.chat_id, msg_id, emoji::THUMBS_UP).await;
        }

        // Update message to "Done!"
        let status = OperationStatus::Success {
            info: self.inner.info.clone(),
            message: message.map(String::from),
        };
        self.update_message(&status).await?;
        self.inner.current_status = Some(status);

        // Schedule message clear
        if clear_after_secs > 0 {
            let bot = self.inner.bot.clone();
            let chat_id = self.inner.chat_id;
            let progress_msg_id = self.inner.progress_message_id;
            let formatter = MarkdownV2Formatter;
            let final_msg = formatter.format_final(&OperationStatus::Success {
                info: self.inner.info.clone(),
                message: None,
            });

            let handle = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(clear_after_secs)).await;
                if let Some(msg_id) = progress_msg_id {
                    let _ = bot
                        .edit_message_text(chat_id, msg_id, final_msg)
                        .parse_mode(ParseMode::MarkdownV2)
                        .await;
                }
            });
            self.inner.clear_task = Some(handle);
        }

        Ok(Operation {
            inner: self.inner,
            _state: PhantomData,
        })
    }

    /// Completes operation with error.
    ///
    /// Transitions from `InProgress` to `Completed` state.
    pub async fn complete_error(mut self, error: &str) -> anyhow::Result<Operation<Completed>> {
        self.inner.cancel_clear_task();

        // Set error reaction
        if let Some(msg_id) = self.inner.user_message_id {
            try_set_reaction(&self.inner.bot, self.inner.chat_id, msg_id, emoji::SAD).await;
        }

        // Update message with error
        let status = OperationStatus::Error {
            info: self.inner.info.clone(),
            error: error.to_string(),
        };
        self.update_message(&status).await?;
        self.inner.current_status = Some(status);

        Ok(Operation {
            inner: self.inner,
            _state: PhantomData,
        })
    }

    /// Deletes the progress message.
    pub async fn delete_progress_message(&mut self) -> anyhow::Result<()> {
        self.inner.cancel_clear_task();

        if let Some(msg_id) = self.inner.progress_message_id.take() {
            if let Err(e) = self.inner.bot.delete_message(self.inner.chat_id, msg_id).await {
                let err_str = e.to_string();
                if !err_str.contains("message to delete not found") && !err_str.contains("MESSAGE_ID_INVALID") {
                    return Err(e.into());
                }
            }
        }
        Ok(())
    }

    /// Returns the chat ID.
    #[must_use]
    pub fn chat_id(&self) -> ChatId {
        self.inner.chat_id
    }

    /// Returns the progress message ID.
    #[must_use]
    pub fn progress_message_id(&self) -> Option<MessageId> {
        self.inner.progress_message_id
    }

    /// Returns current progress percentage (if in Progress status).
    #[must_use]
    pub fn current_progress(&self) -> Option<u8> {
        match &self.inner.current_status {
            Some(OperationStatus::Progress { progress, .. }) => Some(*progress),
            _ => None,
        }
    }

    /// Returns reference to current status.
    #[must_use]
    pub fn status(&self) -> Option<&OperationStatus> {
        self.inner.current_status.as_ref()
    }

    /// Returns the operation info.
    #[must_use]
    pub fn info(&self) -> &OperationInfo {
        &self.inner.info
    }

    // --- Private helpers ---

    fn should_update(&self) -> bool {
        match self.inner.last_update {
            Some(last) => last.elapsed() >= self.inner.throttle_interval,
            None => true,
        }
    }

    async fn update_message(&mut self, status: &OperationStatus) -> anyhow::Result<()> {
        let formatter = MarkdownV2Formatter;
        let text = formatter.format(status);

        if let Some(msg_id) = self.inner.progress_message_id {
            match self
                .inner
                .bot
                .edit_message_text(self.inner.chat_id, msg_id, text.clone())
                .parse_mode(ParseMode::MarkdownV2)
                .await
            {
                Ok(_) => Ok(()),
                Err(e) => {
                    let error_str = e.to_string();
                    if error_str.contains("message is not modified") {
                        return Ok(());
                    }
                    if let Some(retry_secs) = extract_retry_after(&error_str) {
                        log::warn!("Rate limited, waiting {}s before retry", retry_secs);
                        tokio::time::sleep(Duration::from_secs(retry_secs + 1)).await;
                        match self
                            .inner
                            .bot
                            .edit_message_text(self.inner.chat_id, msg_id, text)
                            .parse_mode(ParseMode::MarkdownV2)
                            .await
                        {
                            Ok(_) => return Ok(()),
                            Err(e2) => {
                                if e2.to_string().contains("message is not modified") {
                                    return Ok(());
                                }
                                log::warn!("Failed to edit message after retry: {}", e2);
                                return Ok(());
                            }
                        }
                    }
                    Err(e.into())
                }
            }
        } else {
            let msg = self
                .inner
                .bot
                .send_message(self.inner.chat_id, text)
                .parse_mode(ParseMode::MarkdownV2)
                .await?;
            self.inner.progress_message_id = Some(msg.id);
            Ok(())
        }
    }
}

// --- Completed State ---

impl Operation<Completed> {
    /// Returns the chat ID.
    #[must_use]
    pub fn chat_id(&self) -> ChatId {
        self.inner.chat_id
    }

    /// Returns the progress message ID.
    #[must_use]
    pub fn progress_message_id(&self) -> Option<MessageId> {
        self.inner.progress_message_id
    }

    /// Returns reference to final status.
    #[must_use]
    pub fn status(&self) -> Option<&OperationStatus> {
        self.inner.current_status.as_ref()
    }

    /// Returns the operation info.
    #[must_use]
    pub fn info(&self) -> &OperationInfo {
        &self.inner.info
    }

    /// Cancels the scheduled clear task (if any).
    pub fn cancel_clear_task(&mut self) {
        self.inner.cancel_clear_task();
    }

    /// Deletes the progress message.
    pub async fn delete_progress_message(&mut self) -> anyhow::Result<()> {
        self.inner.cancel_clear_task();

        if let Some(msg_id) = self.inner.progress_message_id.take() {
            if let Err(e) = self.inner.bot.delete_message(self.inner.chat_id, msg_id).await {
                let err_str = e.to_string();
                if !err_str.contains("message to delete not found") && !err_str.contains("MESSAGE_ID_INVALID") {
                    return Err(e.into());
                }
            }
        }
        Ok(())
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Creates a visual progress bar.
fn create_progress_bar(progress: u8) -> String {
    let progress = progress.min(100);
    let filled = (progress / 10) as usize;
    let empty = 10 - filled;
    format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
}

/// Extracts retry-after seconds from Telegram rate limit error.
fn extract_retry_after(error_str: &str) -> Option<u64> {
    let lower = error_str.to_lowercase();

    if let Some(pos) = lower.find("retry after ") {
        let after = &lower[pos + 12..];
        let num: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(secs) = num.parse() {
            return Some(secs);
        }
    }

    if let Some(pos) = lower.find("retry_after") {
        let after = &lower[pos + 11..];
        let num: String = after
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(secs) = num.parse() {
            return Some(secs);
        }
    }

    None
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_bar() {
        assert_eq!(create_progress_bar(0), "[░░░░░░░░░░]");
        assert_eq!(create_progress_bar(50), "[█████░░░░░]");
        assert_eq!(create_progress_bar(100), "[██████████]");
        assert_eq!(create_progress_bar(150), "[██████████]");
    }

    #[test]
    fn test_operation_info_default() {
        let info = OperationInfo::default();
        assert_eq!(info.title, "Operation");
        assert_eq!(info.emoji, DEFAULT_EMOJI);
    }

    #[test]
    fn test_operation_info_new() {
        let info = OperationInfo::new("Test", Some("🎥"));
        assert_eq!(info.title, "Test");
        assert_eq!(info.emoji, "🎥");

        let info2 = OperationInfo::new("Test2", None);
        assert_eq!(info2.emoji, DEFAULT_EMOJI);
    }

    #[test]
    fn test_status_starting() {
        let status = OperationStatus::Starting(OperationInfo::new("Test", Some("🎥")));
        let formatter = MarkdownV2Formatter;
        let msg = formatter.format(&status);
        assert!(msg.contains("Test"));
        assert!(msg.contains("🎥"));
        assert!(msg.contains("⏳"));
        assert!(msg.contains("Starting"));
    }

    #[test]
    fn test_status_progress() {
        let status = OperationStatus::Progress {
            info: OperationInfo::new("Test", None),
            progress: 50,
            stage: Some("Processing".to_string()),
        };
        let formatter = MarkdownV2Formatter;
        let msg = formatter.format(&status);
        assert!(msg.contains("50%"));
        assert!(msg.contains("[█████░░░░░]"));
        assert!(msg.contains("Processing"));
    }

    #[test]
    fn test_status_working() {
        let status = OperationStatus::Working {
            info: OperationInfo::new("Test", None),
            stage: Some("Converting".to_string()),
            dots: 2,
        };
        let formatter = MarkdownV2Formatter;
        let msg = formatter.format(&status);
        assert!(msg.contains("Working"));
        assert!(msg.contains("Converting"));
    }

    #[test]
    fn test_status_success() {
        let status = OperationStatus::Success {
            info: OperationInfo::new("Test", None),
            message: Some("All done!".to_string()),
        };
        let formatter = MarkdownV2Formatter;
        let msg = formatter.format(&status);
        assert!(msg.contains("✅"));
        assert!(msg.contains("Done"));
        assert!(msg.contains("All done"));
    }

    #[test]
    fn test_status_error() {
        let status = OperationStatus::Error {
            info: OperationInfo::new("Test", None),
            error: "Something failed".to_string(),
        };
        let formatter = MarkdownV2Formatter;
        let msg = formatter.format(&status);
        assert!(msg.contains("❌"));
        assert!(msg.contains("Error"));
        assert!(msg.contains("Something failed"));
    }

    #[test]
    fn test_status_is_terminal() {
        assert!(!OperationStatus::Starting(OperationInfo::default()).is_terminal());
        assert!(!OperationStatus::Progress {
            info: OperationInfo::default(),
            progress: 50,
            stage: None
        }
        .is_terminal());
        assert!(OperationStatus::Success {
            info: OperationInfo::default(),
            message: None
        }
        .is_terminal());
        assert!(OperationStatus::Error {
            info: OperationInfo::default(),
            error: "err".to_string()
        }
        .is_terminal());
    }

    #[test]
    fn test_final_message() {
        let status = OperationStatus::Success {
            info: OperationInfo::new("Test", Some("🎥")),
            message: Some("Done!".to_string()),
        };
        let formatter = MarkdownV2Formatter;
        let msg = formatter.format_final(&status);
        assert_eq!(msg, "🎥 *Test*");
    }

    #[test]
    fn test_extract_retry_after() {
        assert_eq!(extract_retry_after("Retry after 30s"), Some(30));
        assert_eq!(extract_retry_after("retry after 60s"), Some(60));
        assert_eq!(extract_retry_after("retry_after: 45"), Some(45));
        assert_eq!(extract_retry_after("retry_after:30"), Some(30));
        assert_eq!(extract_retry_after("No retry info"), None);
        assert_eq!(extract_retry_after(""), None);
    }

    #[test]
    fn test_plain_text_formatter() {
        let status = OperationStatus::Progress {
            info: OperationInfo::new("Download", Some("📥")),
            progress: 75,
            stage: Some("Fetching".to_string()),
        };
        let formatter = PlainTextFormatter;
        let msg = formatter.format(&status);
        assert_eq!(msg, "📥 Download - Progress: 75% (Fetching)");
    }

    #[test]
    fn test_error_display() {
        let err = OperationError::Other(anyhow::anyhow!("test error"));
        assert_eq!(err.to_string(), "test error");
    }
}
