//! Platform-neutral messaging types. No teloxide, no platform SDKs — these are
//! the lingua franca the core flows speak; each platform adapter translates
//! to/from its own wire format.

use serde::{Deserialize, Serialize};

/// A messaging platform an adapter serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Platform {
    Telegram,
    WhatsApp,
    IMessage,
    X,
}

impl Platform {
    /// Stable lowercase id (storage key, `user_identities.platform`, logging).
    pub fn id(self) -> &'static str {
        match self {
            Platform::Telegram => "telegram",
            Platform::WhatsApp => "whatsapp",
            Platform::IMessage => "imessage",
            Platform::X => "x",
        }
    }

    pub fn from_id(s: &str) -> Option<Self> {
        match s {
            "telegram" => Some(Platform::Telegram),
            "whatsapp" => Some(Platform::WhatsApp),
            "imessage" => Some(Platform::IMessage),
            "x" => Some(Platform::X),
            _ => None,
        }
    }
}

/// A conversation address. `chat_id` is the platform's native chat/thread id as
/// a string (Telegram chat id, WhatsApp wa_id/phone, iMessage handle, …).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChatRef {
    pub platform: Platform,
    pub chat_id: String,
}

/// A user address (the sender). Often equals the chat in 1:1 DMs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserRef {
    pub platform: Platform,
    pub user_id: String,
}

/// What a piece of media is, for adapters to pick the right send primitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    Audio,
    Video,
    VideoNote,
    Photo,
    Animation,
    Document,
}

/// Where the bytes of an outgoing media come from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaSource {
    /// A file on local disk (the just-downloaded result).
    LocalPath(String),
    /// A public URL the platform fetches itself.
    Url(String),
    /// A platform-cached reference (Telegram `file_id`, WhatsApp `media_id`, …)
    /// — instant re-send without re-upload. Opaque per platform.
    CachedRef(String),
}

/// How the text body should be interpreted by the adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextStyle {
    Plain,
    Markdown,
    Html,
}

/// One tappable choice. `action` is the opaque routing token (today's
/// callback-data string, e.g. `"exp:tab:recent"`); adapters that can't carry
/// callbacks map it to a numbered reply the [`crate::messaging::InboundSource`]
/// resolves back to an `Action`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Button {
    pub label: String,
    pub action: String,
}

impl Button {
    pub fn new(label: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            action: action.into(),
        }
    }
}

/// A grid of buttons (rows × columns). Rich platforms render an inline keyboard;
/// limited platforms degrade per [`Capabilities`](crate::messaging::Capabilities).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keyboard {
    pub rows: Vec<Vec<Button>>,
}

impl Keyboard {
    pub fn new(rows: Vec<Vec<Button>>) -> Self {
        Self { rows }
    }

    pub fn is_empty(&self) -> bool {
        self.rows.iter().all(|r| r.is_empty())
    }

    /// Flatten to a single list of buttons (used by numbered-text fallback).
    pub fn flat(&self) -> Vec<&Button> {
        self.rows.iter().flatten().collect()
    }

    /// Re-flow all buttons into rows of at most `per_row` (clamped to ≥1) — used
    /// by adapters whose `max_buttons_per_row` is smaller than the author's
    /// layout (e.g. WhatsApp ≤ 3).
    pub fn reflow(&self, per_row: usize) -> Keyboard {
        let per_row = per_row.max(1);
        let flat: Vec<Button> = self.rows.iter().flatten().cloned().collect();
        Keyboard {
            rows: flat.chunks(per_row).map(<[Button]>::to_vec).collect(),
        }
    }

    /// Total button count.
    pub fn len(&self) -> usize {
        self.rows.iter().map(Vec::len).sum()
    }
}

/// A message to send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutboundMessage {
    Text {
        body: String,
        style: TextStyle,
        keyboard: Option<Keyboard>,
    },
    Media {
        kind: MediaKind,
        source: MediaSource,
        caption: Option<String>,
        style: TextStyle,
        keyboard: Option<Keyboard>,
    },
}

/// Opaque handle to a sent message, for later edit/delete. The adapter encodes
/// whatever it needs (chat + message id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageHandle {
    pub platform: Platform,
    pub chat_id: String,
    pub message_id: String,
}

/// A normalized inbound event from any platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InboundEvent {
    /// Free text (likely contains a URL to download, or a prompted reply).
    Text { body: String },
    /// A button press / callback resolved to its routing token.
    Action { id: String },
    /// An uploaded document/file (e.g. cookies upload).
    Document {
        file_ref: String,
        file_name: Option<String>,
        mime: Option<String>,
    },
}

/// One normalized inbound message: who, where, what.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboundMessage {
    pub chat: ChatRef,
    pub user: UserRef,
    pub event: InboundEvent,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_id_roundtrip() {
        for p in [Platform::Telegram, Platform::WhatsApp, Platform::IMessage, Platform::X] {
            assert_eq!(Platform::from_id(p.id()), Some(p));
        }
        assert_eq!(Platform::from_id("nope"), None);
    }

    #[test]
    fn keyboard_reflow_respects_per_row() {
        let kb = Keyboard::new(vec![vec![
            Button::new("1", "a"),
            Button::new("2", "b"),
            Button::new("3", "c"),
            Button::new("4", "d"),
            Button::new("5", "e"),
        ]]);
        let r = kb.reflow(3);
        assert_eq!(r.rows.len(), 2);
        assert_eq!(r.rows[0].len(), 3);
        assert_eq!(r.rows[1].len(), 2);
        assert_eq!(r.len(), 5);
        // per_row 0 is clamped to 1 (no panic / empty rows).
        assert_eq!(kb.reflow(0).rows.len(), 5);
    }

    #[test]
    fn keyboard_flat_preserves_order() {
        let kb = Keyboard::new(vec![vec![Button::new("a", "1")], vec![Button::new("b", "2")]]);
        let flat = kb.flat();
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[0].action, "1");
        assert_eq!(flat[1].action, "2");
    }
}
