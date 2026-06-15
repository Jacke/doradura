//! Per-platform capability descriptors. The core renders the best UI it can for
//! a given platform by consulting these — instead of hard-coding Telegram
//! assumptions (5-button rows, edit-in-place, HTML) everywhere.

use super::types::TextStyle;

/// What a platform's messaging surface can do. Drives UI degradation
/// (inline keyboard → reply buttons → list → numbered text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// True if the platform supports tap-to-callback buttons attached to a
    /// message (Telegram inline keyboard, WhatsApp reply buttons). When false,
    /// buttons are rendered as a numbered text menu and replies are matched back.
    pub inline_keyboard: bool,
    /// True if an already-sent message can be edited in place (Telegram). When
    /// false, "edits" become a freshly sent message.
    pub edit_in_place: bool,
    /// Max buttons per row the platform renders (Telegram is effectively
    /// unbounded → large value; WhatsApp reply buttons = 3).
    pub max_buttons_per_row: usize,
    /// Max total tappable buttons on one message before falling back to a list
    /// or numbered text (WhatsApp reply buttons = 3; list rows = 10).
    pub max_buttons_total: usize,
    /// Max rows in a single-select "list" message, or 0 if lists are unsupported.
    pub list_menu_max: usize,
    /// Richest text style the platform renders.
    pub text_style: TextStyle,
    /// Soft media-size ceilings in bytes; downloads above these must be sent as
    /// a document or delivered as a hosted link. `u64::MAX` = effectively none.
    pub max_audio_bytes: u64,
    pub max_video_bytes: u64,
    pub max_document_bytes: u64,
    /// True if the platform allows unsolicited (proactive) messages at any time.
    /// When false (e.g. WhatsApp's 24h window), proactive sends need a template.
    pub proactive_anytime: bool,
}

impl Capabilities {
    /// Telegram (local Bot API server): rich keyboards, edit-in-place, HTML,
    /// huge media limits, proactive anytime.
    pub const TELEGRAM: Capabilities = Capabilities {
        inline_keyboard: true,
        edit_in_place: true,
        max_buttons_per_row: 8,
        max_buttons_total: 100,
        list_menu_max: 0,
        text_style: TextStyle::Html,
        // Local Bot API server lifts the 50MB cap to ~2GB.
        max_audio_bytes: 2_000_000_000,
        max_video_bytes: 2_000_000_000,
        max_document_bytes: 2_000_000_000,
        proactive_anytime: true,
    };

    /// WhatsApp Cloud API: ≤3 reply buttons or ≤10-row list, no edit-in-place,
    /// plain/markdown only, tight media caps, 24h proactive window.
    pub const WHATSAPP: Capabilities = Capabilities {
        inline_keyboard: true,
        edit_in_place: false,
        max_buttons_per_row: 1, // reply buttons stack vertically
        max_buttons_total: 3,
        list_menu_max: 10,
        text_style: TextStyle::Markdown, // WhatsApp uses *bold*/_italic_, not HTML
        max_audio_bytes: 16_000_000,
        max_video_bytes: 16_000_000,
        max_document_bytes: 100_000_000,
        proactive_anytime: false,
    };

    /// iMessage via a 3rd-party provider: essentially plain text + media; no
    /// reliable interactive buttons → numbered-text menus.
    pub const IMESSAGE: Capabilities = Capabilities {
        inline_keyboard: false,
        edit_in_place: false,
        max_buttons_per_row: 1,
        max_buttons_total: 0,
        list_menu_max: 0,
        text_style: TextStyle::Plain,
        max_audio_bytes: 100_000_000,
        max_video_bytes: 100_000_000,
        max_document_bytes: 100_000_000,
        proactive_anytime: true,
    };

    /// Whether buttons should be rendered as a numbered-text menu rather than
    /// native tappable buttons (no inline support, or more buttons than the
    /// platform allows natively).
    pub fn needs_text_menu(&self, button_count: usize) -> bool {
        if !self.inline_keyboard {
            return true;
        }
        let native_cap = self.max_buttons_total.max(self.list_menu_max);
        button_count > native_cap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telegram_keeps_native_keyboard() {
        assert!(!Capabilities::TELEGRAM.needs_text_menu(12));
    }

    #[test]
    fn whatsapp_uses_list_then_text_menu() {
        // ≤3 → reply buttons; ≤10 → list; >10 → numbered text.
        assert!(!Capabilities::WHATSAPP.needs_text_menu(3));
        assert!(!Capabilities::WHATSAPP.needs_text_menu(10));
        assert!(Capabilities::WHATSAPP.needs_text_menu(11));
    }

    #[test]
    fn imessage_always_text_menu() {
        assert!(Capabilities::IMESSAGE.needs_text_menu(1));
    }
}
