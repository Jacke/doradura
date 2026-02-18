//! Core trait and types for the content watcher system.
//!
//! The `ContentWatcher` trait defines how a source (Instagram, YouTube, etc.)
//! checks for new content. The watcher module has zero teloxide dependency —
//! notifications are emitted as plain structs through an mpsc channel.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A single new content item detected by a watcher.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchUpdate {
    /// Type of content: "post", "story", etc.
    pub content_type: String,
    /// URL to the content (e.g. `https://instagram.com/p/ABC123/`)
    pub url: String,
    /// Short human-readable description
    pub description: String,
}

/// Result of a check operation.
#[derive(Debug)]
pub struct CheckResult {
    /// New content found since last check (empty on first check).
    pub updates: Vec<WatchUpdate>,
    /// Updated state to persist (replaces `last_seen_state` in DB).
    pub new_state: serde_json::Value,
    /// Updated source metadata (e.g. `ig_user_id`), if changed.
    pub new_meta: Option<serde_json::Value>,
}

/// Notification sent through the mpsc channel to the Telegram layer.
#[derive(Debug, Clone)]
pub struct WatchNotification {
    pub user_id: i64,
    pub source_type: String,
    pub source_id: String,
    pub display_name: String,
    pub subscription_id: i64,
    pub update: WatchUpdate,
}

/// Trait implemented by each content source (Instagram, YouTube, etc.).
///
/// All methods are `&self` — implementations should be stateless or use
/// interior mutability. The scheduler calls `check()` once per unique
/// (source_type, source_id) and fans out notifications to all subscribers.
#[async_trait]
pub trait ContentWatcher: Send + Sync {
    /// Unique source type identifier (e.g. "instagram").
    fn source_type(&self) -> &str;

    /// Human-readable display name (e.g. "Instagram").
    fn display_name(&self) -> &str;

    /// Available content types with their bitmask values.
    /// Example: `[(1, "Posts"), (2, "Stories")]`
    fn content_types(&self) -> Vec<(u32, &str)>;

    /// Default watch mask for new subscriptions.
    fn default_watch_mask(&self) -> u32 {
        3
    }

    /// Check for new content since `last_state`.
    ///
    /// When `last_state` is `None` (first check), populate state but
    /// emit NO updates to avoid flooding with existing content.
    async fn check(
        &self,
        source_id: &str,
        watch_mask: u32,
        last_state: Option<&serde_json::Value>,
        source_meta: Option<&serde_json::Value>,
    ) -> Result<CheckResult, String>;

    /// Resolve a user-provided source ID (e.g. username) to a canonical form.
    /// Returns `(display_name, optional_source_meta)`.
    async fn resolve_source(&self, source_id: &str) -> Result<(String, Option<serde_json::Value>), String>;

    /// Estimated number of API requests needed per check for this watch mask.
    fn requests_per_check(&self, watch_mask: u32) -> u32 {
        watch_mask.count_ones()
    }
}
