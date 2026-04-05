//! Shared download context — groups parameters common to all three download entry points.
//!
//! Instead of passing the same nine arguments to every `download_and_send_*` function,
//! callers construct a [`DownloadContext`] once and pass it through.
//!
//! # Example
//!
//! ```rust,ignore
//! use crate::download::context::DownloadContext;
//!
//! let ctx = DownloadContext {
//!     bot,
//!     chat_id,
//!     url,
//!     rate_limiter,
//!     db_pool: Some(Arc::clone(&sqlite_pool)),
//!     shared_storage: Some(Arc::clone(&shared_storage)),
//!     message_id: task.message_id,
//!     alert_manager: alert_manager.clone(),
//!     created_timestamp: task.created_timestamp,
//! };
//! download_and_send_audio(ctx, audio_bitrate, time_range, with_lyrics).await
//! ```

use crate::core::alerts::AlertManager;
use crate::core::rate_limiter::RateLimiter;
use crate::storage::db::DbPool;
use crate::storage::SharedStorage;
use crate::telegram::Bot;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use teloxide::prelude::ChatId;
use url::Url;

/// Parameters shared by every `download_and_send_*` function.
///
/// Construct once in the queue processor and pass by value into each dispatch branch.
/// Format-specific parameters (bitrate, quality, subtitle format, etc.) remain as
/// individual arguments on each function so the per-format API stays explicit.
pub struct DownloadContext {
    /// Telegram bot handle used for progress messages and file delivery.
    pub bot: Bot,
    /// Chat (user) the download belongs to.
    pub chat_id: ChatId,
    /// Source URL to download from.
    pub url: Url,
    /// Rate-limiter shared across the application.
    pub rate_limiter: Arc<RateLimiter>,
    /// SQLite connection pool forwarded from the queue processor.
    /// `None` in unit tests or contexts where only `SharedStorage` is available.
    pub db_pool: Option<Arc<DbPool>>,
    /// Redis-backed shared storage for user state, history, and settings.
    pub shared_storage: Option<Arc<SharedStorage>>,
    /// Telegram message ID of the user's original request, used for reactions.
    pub message_id: Option<i32>,
    /// Alert manager for forwarding critical errors to the admin channel.
    pub alert_manager: Option<Arc<AlertManager>>,
    /// When the task was enqueued. Kept for API compatibility / future use.
    pub created_timestamp: DateTime<Utc>,
}
