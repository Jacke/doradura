//! Platform-neutral messaging layer (cross-platform support — Phase 0).
//!
//! The bot's core flows (download → send, menus, preview) target these traits
//! instead of teloxide directly, so the same logic runs on Telegram, WhatsApp,
//! iMessage, etc. Each platform provides an adapter implementing [`Messenger`]
//! (outbound) and [`InboundSource`] (normalize incoming webhook/update JSON).
//!
//! This module is intentionally teloxide-free (it lives in `doracore`). The
//! Telegram adapter (in `dorabot`) wraps teloxide to implement these traits;
//! see the roadmap at `docs`/the cross-platform plan.

pub mod capabilities;
pub mod types;

pub use capabilities::Capabilities;
pub use types::{
    Button, ChatRef, InboundEvent, InboundMessage, Keyboard, MediaKind, MediaSource, MessageHandle, OutboundMessage,
    Platform, TextStyle, UserRef,
};

use async_trait::async_trait;

/// Outbound side of a platform adapter: send/edit/delete messages and media.
/// Implementors translate neutral [`OutboundMessage`] into platform calls and
/// declare what the platform can do via [`Messenger::capabilities`].
#[async_trait]
pub trait Messenger: Send + Sync {
    /// What this platform's surface supports (drives UI degradation).
    fn capabilities(&self) -> &Capabilities;

    /// Send a message to a chat, returning a handle for later edit/delete.
    async fn send(&self, chat: &ChatRef, message: OutboundMessage) -> anyhow::Result<MessageHandle>;

    /// Edit a previously-sent text message in place. Adapters whose platform
    /// lacks edit-in-place (`capabilities().edit_in_place == false`) should send
    /// a new message and return its handle instead.
    async fn edit_text(
        &self,
        handle: &MessageHandle,
        body: String,
        style: TextStyle,
        keyboard: Option<Keyboard>,
    ) -> anyhow::Result<MessageHandle>;

    /// Delete a previously-sent message (best-effort; ignore "already gone").
    async fn delete(&self, handle: &MessageHandle) -> anyhow::Result<()>;
}

/// Inbound side of a platform adapter: turn a raw platform webhook/update body
/// into zero or more normalized [`InboundMessage`]s the core dispatcher routes.
pub trait InboundSource: Send + Sync {
    /// Which platform this source decodes.
    fn platform(&self) -> Platform;

    /// Normalize a raw inbound payload (already-parsed JSON) into neutral
    /// messages. One webhook delivery may carry several (WhatsApp batches).
    fn normalize(&self, raw: &serde_json::Value) -> Vec<InboundMessage>;
}
