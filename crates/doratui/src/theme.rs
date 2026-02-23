//! Catppuccin theme colours for the dora TUI.
//!
//! The active theme is stored on `App::theme` and initialised from
//! `settings.theme_flavour`. Use `app.theme.*` in all renderer code.
//! The legacy constants remain for any code that hasn't been migrated yet.

use ratatui::style::Color;
use serde::{Deserialize, Serialize};

// ── Flavour ───────────────────────────────────────────────────────────────────

/// Catppuccin colour flavour.  Cycled in the Settings → Appearance section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CatppuccinFlavour {
    /// Dark (the original dora theme).
    #[default]
    Mocha,
    Macchiato,
    Frappe,
    /// Light mode.
    Latte,
}

impl CatppuccinFlavour {
    /// Advance to the next flavour: Mocha → Macchiato → Frappe → Latte → Mocha.
    #[allow(dead_code)]
    pub fn next(self) -> Self {
        match self {
            Self::Mocha => Self::Macchiato,
            Self::Macchiato => Self::Frappe,
            Self::Frappe => Self::Latte,
            Self::Latte => Self::Mocha,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Mocha => "Mocha",
            Self::Macchiato => "Macchiato",
            Self::Frappe => "Frappe",
            Self::Latte => "Latte",
        }
    }

    /// Parse from label string (used by the settings cycle mechanism).
    pub fn from_label(s: &str) -> Self {
        match s {
            "Macchiato" => Self::Macchiato,
            "Frappe" => Self::Frappe,
            "Latte" => Self::Latte,
            _ => Self::Mocha,
        }
    }
}

// ── Colour set ────────────────────────────────────────────────────────────────

/// All UI colours for a single Catppuccin flavour.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct ThemeColors {
    pub base: Color,
    pub crust: Color,
    pub surface0: Color,
    pub surface1: Color,
    pub text: Color,
    pub subtext: Color,
    pub lavender: Color,
    pub mauve: Color,
    pub green: Color,
    pub red: Color,
    pub yellow: Color,
    pub blue: Color,
    pub peach: Color,
    pub teal: Color,
}

/// Build a `ThemeColors` palette for the given flavour.
pub fn palette(flavour: CatppuccinFlavour) -> ThemeColors {
    match flavour {
        CatppuccinFlavour::Mocha => ThemeColors {
            base: Color::Rgb(30, 30, 46),
            crust: Color::Rgb(17, 17, 27),
            surface0: Color::Rgb(49, 50, 68),
            surface1: Color::Rgb(69, 71, 90),
            text: Color::Rgb(205, 214, 244),
            subtext: Color::Rgb(166, 173, 200),
            lavender: Color::Rgb(180, 190, 254),
            mauve: Color::Rgb(203, 166, 247),
            green: Color::Rgb(166, 227, 161),
            red: Color::Rgb(243, 139, 168),
            yellow: Color::Rgb(249, 226, 175),
            blue: Color::Rgb(137, 180, 250),
            peach: Color::Rgb(250, 179, 135),
            teal: Color::Rgb(148, 226, 213),
        },
        CatppuccinFlavour::Macchiato => ThemeColors {
            base: Color::Rgb(36, 39, 58),
            crust: Color::Rgb(24, 25, 38),
            surface0: Color::Rgb(54, 58, 79),
            surface1: Color::Rgb(73, 77, 100),
            text: Color::Rgb(202, 211, 245),
            subtext: Color::Rgb(165, 173, 206),
            lavender: Color::Rgb(183, 189, 248),
            mauve: Color::Rgb(198, 160, 246),
            green: Color::Rgb(166, 218, 149),
            red: Color::Rgb(237, 135, 150),
            yellow: Color::Rgb(238, 212, 159),
            blue: Color::Rgb(138, 173, 244),
            peach: Color::Rgb(245, 169, 127),
            teal: Color::Rgb(139, 213, 202),
        },
        CatppuccinFlavour::Frappe => ThemeColors {
            base: Color::Rgb(48, 52, 70),
            crust: Color::Rgb(35, 38, 52),
            surface0: Color::Rgb(65, 69, 89),
            surface1: Color::Rgb(81, 87, 109),
            text: Color::Rgb(198, 208, 245),
            subtext: Color::Rgb(165, 173, 206),
            lavender: Color::Rgb(186, 187, 241),
            mauve: Color::Rgb(202, 158, 230),
            green: Color::Rgb(166, 209, 137),
            red: Color::Rgb(231, 130, 132),
            yellow: Color::Rgb(229, 200, 144),
            blue: Color::Rgb(140, 170, 238),
            peach: Color::Rgb(239, 159, 118),
            teal: Color::Rgb(129, 200, 190),
        },
        CatppuccinFlavour::Latte => ThemeColors {
            base: Color::Rgb(239, 241, 245),
            crust: Color::Rgb(220, 224, 232),
            surface0: Color::Rgb(204, 208, 218),
            surface1: Color::Rgb(188, 192, 204),
            text: Color::Rgb(76, 79, 105),
            subtext: Color::Rgb(100, 102, 134),
            lavender: Color::Rgb(114, 135, 253),
            mauve: Color::Rgb(136, 57, 239),
            green: Color::Rgb(64, 160, 43),
            red: Color::Rgb(210, 15, 57),
            yellow: Color::Rgb(223, 142, 29),
            blue: Color::Rgb(30, 102, 245),
            peach: Color::Rgb(254, 100, 11),
            teal: Color::Rgb(23, 146, 153),
        },
    }
}

// ── Logo colour scheme ─────────────────────────────────────────────────────────

/// Logo colour scheme, cycled on logo click.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LogoScheme {
    #[default]
    Catppuccin,
    Fire,
    Ice,
    Matrix,
    Sunset,
    Neon,
    Gold,
}

impl LogoScheme {
    pub fn next(self) -> Self {
        match self {
            Self::Catppuccin => Self::Fire,
            Self::Fire => Self::Ice,
            Self::Ice => Self::Matrix,
            Self::Matrix => Self::Sunset,
            Self::Sunset => Self::Neon,
            Self::Neon => Self::Gold,
            Self::Gold => Self::Catppuccin,
        }
    }

    pub fn tagline(self) -> &'static str {
        match self {
            Self::Catppuccin => "The Ultimate Media Downloader \u{b7} yt-dlp + ffmpeg",
            Self::Fire => "\u{1f525}  Burn your bandwidth  \u{b7}  yt-dlp + ffmpeg",
            Self::Ice => "\u{2744}\u{fe0f}  Ice-cold downloads  \u{b7}  yt-dlp + ffmpeg",
            Self::Matrix => "\u{2593}  Follow the white rabbit  \u{b7}  yt-dlp + ffmpeg",
            Self::Sunset => "\u{1f305}  Sunset vibes  \u{b7}  yt-dlp + ffmpeg",
            Self::Neon => "\u{26a1}  Neon overdrive  \u{b7}  yt-dlp + ffmpeg",
            Self::Gold => "\u{2728}  Golden ratio downloads  \u{b7}  yt-dlp + ffmpeg",
        }
    }
}

// ── Legacy constants (Mocha) — used by logo.rs static palettes ────────────────
#[allow(dead_code)]
pub const CRUST: Color = Color::Rgb(17, 17, 27);
#[allow(dead_code)]
pub const BASE: Color = Color::Rgb(30, 30, 46);
#[allow(dead_code)]
pub const SURFACE0: Color = Color::Rgb(49, 50, 68);
#[allow(dead_code)]
pub const SURFACE1: Color = Color::Rgb(69, 71, 90);
#[allow(dead_code)]
pub const TEXT: Color = Color::Rgb(205, 214, 244);
#[allow(dead_code)]
pub const SUBTEXT: Color = Color::Rgb(166, 173, 200);
pub const LAVENDER: Color = Color::Rgb(180, 190, 254);
pub const GREEN: Color = Color::Rgb(166, 227, 161);
pub const RED: Color = Color::Rgb(243, 139, 168);
pub const YELLOW: Color = Color::Rgb(249, 226, 175);
pub const BLUE: Color = Color::Rgb(137, 180, 250);
pub const PEACH: Color = Color::Rgb(250, 179, 135);
pub const MAUVE: Color = Color::Rgb(203, 166, 247);
pub const TEAL: Color = Color::Rgb(148, 226, 213);
