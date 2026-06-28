//! WhatsApp Cloud API adapter — the platform port of Doradura.
//!
//! Implements the neutral [`Messenger`](crate::messaging::Messenger) (outbound)
//! and [`InboundSource`](crate::messaging::InboundSource) (webhook → events)
//! traits against Meta's WhatsApp Cloud (Graph) API, so the bot's existing,
//! platform-neutral download/convert/send core (in `doracore`) runs on WhatsApp
//! unchanged. This is Phase 2 of the cross-platform roadmap.
//!
//! Layering:
//! - [`config`] — env-driven credentials + URL composition.
//! - [`wire`] — pure builders for outbound JSON payloads (no I/O).
//! - [`format`] — `TextStyle`/HTML → WhatsApp markup + length clamps.
//! - [`mime`] — upload MIME inference.
//! - [`client`] — reqwest I/O: send, media up/download.
//! - [`inbound`] — webhook JSON → neutral events.
//! - [`webhook`] — subscription handshake + `X-Hub-Signature-256` verification.
//!
//! Capability differences from Telegram (≤3 reply buttons / ≤10-row list, no
//! edit-in-place, no message delete, ~16 MB media, WhatsApp markup) are handled
//! here via [`Capabilities::WHATSAPP`] so the core never special-cases platforms.

pub mod client;
pub mod config;
pub mod format;
pub mod inbound;
pub mod mime;
pub mod server;
pub mod webhook;
pub mod wire;

pub use client::WhatsAppClient;
pub use config::WhatsAppConfig;
pub use inbound::WhatsAppInbound;

use async_trait::async_trait;

use crate::messaging::Messenger;
use crate::messaging::capabilities::Capabilities;
use crate::messaging::types::{ChatRef, Keyboard, MediaSource, MessageHandle, OutboundMessage, Platform, TextStyle};

use wire::{KeyboardPlan, MediaRef};

/// Outbound WhatsApp adapter. Cheap to clone (shares the underlying
/// [`WhatsAppClient`]'s connection pool).
#[derive(Clone)]
pub struct WhatsAppMessenger {
    client: WhatsAppClient,
    caps: Capabilities,
}

impl WhatsAppMessenger {
    /// Build from a configured client, using [`Capabilities::WHATSAPP`].
    pub fn new(client: WhatsAppClient) -> Self {
        Self {
            client,
            caps: Capabilities::WHATSAPP,
        }
    }

    /// Build straight from config.
    pub fn from_config(cfg: WhatsAppConfig) -> Self {
        Self::new(WhatsAppClient::new(cfg))
    }

    fn handle(&self, chat_id: &str, message_id: String) -> MessageHandle {
        MessageHandle {
            platform: Platform::WhatsApp,
            chat_id: chat_id.to_string(),
            message_id,
        }
    }

    /// Send a text body, rendering an attached keyboard per WhatsApp
    /// capabilities (reply buttons → list → numbered text). Returns the wamid.
    async fn send_keyed_text(
        &self,
        to: &str,
        body: &str,
        style: TextStyle,
        keyboard: Option<&Keyboard>,
    ) -> anyhow::Result<String> {
        let rendered = format::to_whatsapp_text(body, style);
        let payload = match wire::plan_keyboard(keyboard, &self.caps) {
            KeyboardPlan::None => wire::text_payload(to, &rendered, true),
            KeyboardPlan::Buttons(items) => wire::buttons_payload(to, &rendered, &items),
            KeyboardPlan::List(items) => wire::list_payload(to, &rendered, "Menu", "Options", &items),
            KeyboardPlan::NumberedText(items) => {
                wire::text_payload(to, &wire::numbered_menu_text(&rendered, &items), true)
            }
        };
        self.client.send_message(&payload).await
    }

    /// Resolve a neutral [`MediaSource`] to a wire [`MediaRef`], uploading local
    /// files to obtain a reusable `media_id`.
    async fn resolve_media(
        &self,
        kind: crate::messaging::types::MediaKind,
        source: &MediaSource,
    ) -> anyhow::Result<MediaRef> {
        match source {
            MediaSource::Url(url) => Ok(MediaRef::Link(url.clone())),
            MediaSource::CachedRef(id) => Ok(MediaRef::Id(id.clone())),
            MediaSource::LocalPath(path) => {
                let mime = mime::guess_mime(kind, path);
                let id = self.client.upload_media(path, mime).await?;
                Ok(MediaRef::Id(id))
            }
        }
    }
}

#[async_trait]
impl Messenger for WhatsAppMessenger {
    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    async fn send(&self, chat: &ChatRef, message: OutboundMessage) -> anyhow::Result<MessageHandle> {
        let to = chat.chat_id.as_str();
        match message {
            OutboundMessage::Text { body, style, keyboard } => {
                let id = self.send_keyed_text(to, &body, style, keyboard.as_ref()).await?;
                Ok(self.handle(to, id))
            }
            OutboundMessage::Media {
                kind,
                source,
                caption,
                style,
                keyboard,
            } => {
                let media = self.resolve_media(kind, &source).await?;
                let has_keyboard = keyboard.as_ref().is_some_and(|k| !k.is_empty());

                // WhatsApp media can't carry buttons. When a keyboard is present,
                // send the file *without* caption, then a follow-up message that
                // pairs the caption with the buttons — mirroring Telegram's
                // caption+keyboard-on-media layout as closely as the API allows.
                let caption_for_media = if has_keyboard {
                    None
                } else {
                    caption.as_deref().map(|c| format::to_whatsapp_text(c, style))
                };
                let filename = match &source {
                    MediaSource::LocalPath(p) => std::path::Path::new(p)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(str::to_string),
                    _ => None,
                };

                let payload = wire::media_payload(to, kind, &media, caption_for_media.as_deref(), filename.as_deref());
                let media_id = self.client.send_message(&payload).await?;

                if has_keyboard {
                    let follow_body = caption.unwrap_or_default();
                    // Best-effort: the file already delivered; don't fail the
                    // whole send if only the actions message errors.
                    if let Err(e) = self.send_keyed_text(to, &follow_body, style, keyboard.as_ref()).await {
                        tracing::warn!(error = %e, "whatsapp: media sent but follow-up keyboard failed");
                    }
                }
                Ok(self.handle(to, media_id))
            }
        }
    }

    async fn edit_text(
        &self,
        handle: &MessageHandle,
        body: String,
        style: TextStyle,
        keyboard: Option<Keyboard>,
    ) -> anyhow::Result<MessageHandle> {
        // WhatsApp has no general edit-in-place for business messages
        // (`capabilities().edit_in_place == false`): send a new message instead.
        let id = self
            .send_keyed_text(&handle.chat_id, &body, style, keyboard.as_ref())
            .await?;
        Ok(self.handle(&handle.chat_id, id))
    }

    async fn delete(&self, handle: &MessageHandle) -> anyhow::Result<()> {
        // The Cloud API has no endpoint to delete a business-sent message.
        // Best-effort no-op so callers (e.g. progress-message cleanup) don't err.
        tracing::debug!(message_id = %handle.message_id, "whatsapp: delete is a no-op (unsupported by Cloud API)");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::types::{Button, MediaKind};

    fn messenger() -> WhatsAppMessenger {
        WhatsAppMessenger::from_config(WhatsAppConfig::new("tok", "PID", "vt", "sec"))
    }

    #[test]
    fn capabilities_are_whatsapp() {
        let m = messenger();
        assert!(!m.capabilities().edit_in_place);
        assert_eq!(m.capabilities().max_buttons_total, 3);
        assert_eq!(m.capabilities().text_style, TextStyle::Markdown);
    }

    #[test]
    fn handle_is_whatsapp_scoped() {
        let h = messenger().handle("49151", "wamid.X".into());
        assert_eq!(h.platform, Platform::WhatsApp);
        assert_eq!(h.chat_id, "49151");
        assert_eq!(h.message_id, "wamid.X");
    }

    // Pure-render assertions (no network): confirm the keyboard-plan → payload
    // selection the async path relies on.
    #[test]
    fn text_with_three_buttons_renders_reply_buttons() {
        let caps = Capabilities::WHATSAPP;
        let kb = Keyboard::new(vec![vec![
            Button::new("Audio", "dl:a"),
            Button::new("Video", "dl:v"),
            Button::new("Ringtone", "dl:r"),
        ]]);
        match wire::plan_keyboard(Some(&kb), &caps) {
            KeyboardPlan::Buttons(items) => {
                let v = wire::buttons_payload("49151", "Pick", &items);
                assert_eq!(v["interactive"]["type"], "button");
                assert_eq!(v["interactive"]["action"]["buttons"].as_array().unwrap().len(), 3);
            }
            other => panic!("expected Buttons, got {other:?}"),
        }
    }

    #[test]
    fn media_resolution_for_url_and_cached() {
        // resolve_media is pure for Url/CachedRef (no upload). Use a runtime to
        // drive the async fn without touching the network.
        let m = messenger();
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let link = rt
            .block_on(m.resolve_media(MediaKind::Video, &MediaSource::Url("https://x/y.mp4".into())))
            .unwrap();
        assert_eq!(link, MediaRef::Link("https://x/y.mp4".into()));
        let cached = rt
            .block_on(m.resolve_media(MediaKind::Audio, &MediaSource::CachedRef("MID".into())))
            .unwrap();
        assert_eq!(cached, MediaRef::Id("MID".into()));
    }
}
