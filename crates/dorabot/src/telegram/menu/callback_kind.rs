//! Typed first-segment parser for Telegram inline-button callback data.
//!
//! Callback data is a stringly-typed protocol carried inside `InlineKeyboardButton`.
//! Parse-don't-validate at the boundary: one `FromStr` on the leading `:`-separated
//! token, then typed `match` downstream. Prevents silent routing bugs from typos in
//! prefix strings.
//!
//! Covers every top-level callback prefix dispatched by
//! `callback_router::handle_menu_callback`. Unknown strings return `None`, which
//! the router treats as "no handler" and silently drops.

use strum::EnumString;

#[derive(EnumString, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CallbackKind {
    // Admin group
    #[strum(serialize = "analytics")]
    Analytics,
    #[strum(serialize = "metrics")]
    Metrics,
    #[strum(serialize = "au")]
    Au,
    #[strum(serialize = "admin")]
    Admin,

    // Settings group
    #[strum(serialize = "mode")]
    Mode,
    #[strum(serialize = "main")]
    Main,
    #[strum(serialize = "ext")]
    Ext,
    #[strum(serialize = "subscribe")]
    Subscribe,
    #[strum(serialize = "subscription")]
    Subscription,
    #[strum(serialize = "language")]
    Language,
    #[strum(serialize = "quality")]
    Quality,
    #[strum(serialize = "send_type")]
    SendType,
    #[strum(serialize = "video")]
    Video,
    #[strum(serialize = "bitrate")]
    Bitrate,
    #[strum(serialize = "audio_send_type")]
    AudioSendType,
    #[strum(serialize = "subtitle")]
    Subtitle,
    #[strum(serialize = "pbar_style")]
    PbarStyle,
    #[strum(serialize = "video_send_type")]
    VideoSendType,
    #[strum(serialize = "settings")]
    Settings,
    #[strum(serialize = "back")]
    Back,
    #[strum(serialize = "experimental")]
    Experimental,

    // Shape 1: forwarded CallbackQuery handlers
    #[strum(serialize = "lyr")]
    Lyr,
    #[strum(serialize = "ac")]
    Ac,
    #[strum(serialize = "ae")]
    Ae,

    // Shape 2: direct handlers (per-prefix module)
    #[strum(serialize = "ct")]
    Ct,
    #[strum(serialize = "ig")]
    Ig,
    #[strum(serialize = "cw")]
    Cw,
    #[strum(serialize = "format")]
    Format,
    #[strum(serialize = "dl")]
    Dl,
    #[strum(serialize = "pv")]
    Pv,
    #[strum(serialize = "history")]
    History,
    #[strum(serialize = "export")]
    Export,
    #[strum(serialize = "vfx")]
    Vfx,
    #[strum(serialize = "vp")]
    Vp,
    #[strum(serialize = "sr")]
    Sr,
    #[strum(serialize = "pw")]
    Pw,
    #[strum(serialize = "pl")]
    Pl,
    #[strum(serialize = "vault")]
    Vault,
    #[strum(serialize = "pi")]
    Pi,
    #[strum(serialize = "vl")]
    Vl,
    #[strum(serialize = "ringtone")]
    Ringtone,
    #[strum(serialize = "downloads")]
    Downloads,
    #[strum(serialize = "cuts")]
    Cuts,
    #[strum(serialize = "videos")]
    Videos,
    #[strum(serialize = "convert")]
    Convert,
    #[strum(serialize = "cut_confirm")]
    CutConfirm,
    #[strum(serialize = "dl_cancel")]
    DlCancel,
    #[strum(serialize = "info")]
    Info,
}

impl CallbackKind {
    /// Parses the first `:`-separated segment (or the whole string if no colon).
    /// Returns `None` for unknown kinds — caller falls through to legacy dispatch.
    pub(crate) fn parse(data: &str) -> Option<Self> {
        let head = data.split_once(':').map_or(data, |(h, _)| h);
        head.parse().ok()
    }

    pub(crate) fn is_admin_group(self) -> bool {
        matches!(self, Self::Analytics | Self::Metrics | Self::Au | Self::Admin)
    }

    pub(crate) fn is_settings_group(self) -> bool {
        matches!(
            self,
            Self::Mode
                | Self::Main
                | Self::Ext
                | Self::Subscribe
                | Self::Subscription
                | Self::Language
                | Self::Quality
                | Self::SendType
                | Self::Video
                | Self::Bitrate
                | Self::AudioSendType
                | Self::Subtitle
                | Self::PbarStyle
                | Self::VideoSendType
                | Self::Settings
                | Self::Back
                | Self::Experimental
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_prefix() {
        assert_eq!(CallbackKind::parse("analytics:overview"), Some(CallbackKind::Analytics));
        assert_eq!(CallbackKind::parse("au:list:1"), Some(CallbackKind::Au));
        assert_eq!(CallbackKind::parse("language:set:en-US"), Some(CallbackKind::Language));
        assert_eq!(CallbackKind::parse("back:main"), Some(CallbackKind::Back));
    }

    #[test]
    fn parses_bare_token() {
        // e.g. a hypothetical data == "admin" (no colon). Still parses.
        assert_eq!(CallbackKind::parse("admin"), Some(CallbackKind::Admin));
    }

    #[test]
    fn parses_phase_c_prefixes() {
        assert_eq!(CallbackKind::parse("lyr:toggle:1"), Some(CallbackKind::Lyr));
        assert_eq!(CallbackKind::parse("dl:mp3:abc"), Some(CallbackKind::Dl));
        assert_eq!(CallbackKind::parse("pv:cancel:1"), Some(CallbackKind::Pv));
        assert_eq!(CallbackKind::parse("videos:list"), Some(CallbackKind::Videos));
        assert_eq!(CallbackKind::parse("convert:mp3"), Some(CallbackKind::Convert));
        assert_eq!(
            CallbackKind::parse("ringtone:select:audio:1"),
            Some(CallbackKind::Ringtone)
        );
    }

    #[test]
    fn returns_none_for_unknown() {
        assert_eq!(CallbackKind::parse("gibberish"), None);
        assert_eq!(CallbackKind::parse("nothing:here"), None);
    }

    #[test]
    fn admin_group_membership() {
        assert!(CallbackKind::Analytics.is_admin_group());
        assert!(CallbackKind::Metrics.is_admin_group());
        assert!(CallbackKind::Au.is_admin_group());
        assert!(CallbackKind::Admin.is_admin_group());
        assert!(!CallbackKind::Settings.is_admin_group());
        assert!(!CallbackKind::Mode.is_admin_group());
    }

    #[test]
    fn settings_group_membership() {
        assert!(CallbackKind::Mode.is_settings_group());
        assert!(CallbackKind::Language.is_settings_group());
        assert!(CallbackKind::VideoSendType.is_settings_group());
        assert!(CallbackKind::Back.is_settings_group());
        assert!(!CallbackKind::Admin.is_settings_group());
    }

    #[test]
    fn groups_are_disjoint() {
        for kind in [
            CallbackKind::Analytics,
            CallbackKind::Metrics,
            CallbackKind::Au,
            CallbackKind::Admin,
            CallbackKind::Mode,
            CallbackKind::Main,
            CallbackKind::Ext,
            CallbackKind::Subscribe,
            CallbackKind::Subscription,
            CallbackKind::Language,
            CallbackKind::Quality,
            CallbackKind::SendType,
            CallbackKind::Video,
            CallbackKind::Bitrate,
            CallbackKind::AudioSendType,
            CallbackKind::Subtitle,
            CallbackKind::PbarStyle,
            CallbackKind::VideoSendType,
            CallbackKind::Settings,
            CallbackKind::Back,
        ] {
            assert!(
                !(kind.is_admin_group() && kind.is_settings_group()),
                "kind {:?} cannot be in both groups",
                kind
            );
        }
    }
}
