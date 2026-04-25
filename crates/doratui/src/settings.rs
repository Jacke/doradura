//! Persistent settings for dora, stored in ~/.config/dora/settings.json.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::theme::{CatppuccinFlavour, LogoScheme};

// ── Cycle-able option lists (shared with ui/settings.rs) ─────────────────────

pub const AUDIO_BITRATES: &[&str] = &["320k", "256k", "192k", "128k"];
pub const VIDEO_QUALITIES: &[&str] = &["1080p", "720p", "480p", "360p", "best"];
pub const RATE_LIMITS: &[&str] = &["off", "2M", "5M", "10M"];
pub const FORMATS: &[&str] = &["MP3", "MP4"];
pub const THEME_FLAVOURS: &[&str] = &["Mocha", "Macchiato", "Frappe", "Latte"];

/// All user-configurable settings for the dora TUI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoraSettings {
    // ── yt-dlp ────────────────────────────────────────────────────────────────
    /// Path/name of the yt-dlp binary.
    pub ytdlp_bin: String,
    /// Output folder for downloads (supports `~`).
    pub output_folder: String,
    /// Preferred audio bitrate when extracting MP3.
    pub audio_bitrate: String,
    /// Maximum video height for MP4 downloads.
    pub video_quality: String,
    /// Download rate limit ("off" | "2M" | "5M" | "10M").
    pub rate_limit: String,
    /// Path to a Netscape-format cookies file for yt-dlp.
    pub ytdlp_cookies: String,

    // ── Instagram ─────────────────────────────────────────────────────────────
    /// Path to cookies file used for Instagram downloads.
    pub instagram_cookies: String,
    /// Instagram GraphQL doc_id (leave empty to use default).
    pub instagram_doc_id: String,

    // ── Conversion ────────────────────────────────────────────────────────────
    /// Default download format ("MP3" | "MP4").
    pub default_format: String,
    /// Default MP3 bitrate for conversions.
    pub default_mp3_bitrate: String,
    /// Default MP4 quality for conversions.
    pub default_mp4_quality: String,

    // ── Theme ─────────────────────────────────────────────────────────────────
    /// Active Catppuccin flavour; cycled in Settings → Appearance.
    #[serde(default)]
    pub theme_flavour: CatppuccinFlavour,
    /// Active logo colour scheme; cycles on logo click.
    #[serde(default)]
    pub logo_scheme: LogoScheme,

    // ── Lyrics ───────────────────────────────────────────────────────────────
    /// Genius API client access token.
    #[serde(default)]
    pub genius_token: String,
}

impl Default for DoraSettings {
    fn default() -> Self {
        Self {
            ytdlp_bin: "yt-dlp".to_string(),
            output_folder: "~/Downloads".to_string(),
            audio_bitrate: "320k".to_string(),
            video_quality: "1080p".to_string(),
            rate_limit: "off".to_string(),
            ytdlp_cookies: String::new(),
            instagram_cookies: String::new(),
            instagram_doc_id: String::new(),
            default_format: "MP3".to_string(),
            default_mp3_bitrate: "320k".to_string(),
            default_mp4_quality: "1080p".to_string(),
            theme_flavour: CatppuccinFlavour::Mocha,
            logo_scheme: LogoScheme::default(),
            genius_token: String::new(),
        }
    }
}

impl DoraSettings {
    /// Load from `~/.config/dora/settings.json`; returns `Default` on any error.
    pub fn load() -> Self {
        if let Ok(content) = fs_err::read_to_string(config_path())
            && let Ok(s) = serde_json::from_str(&content)
        {
            return s;
        }
        Self::default()
    }

    /// Save to `~/.config/dora/settings.json`.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            fs_err::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        fs_err::write(path, content)?;
        Ok(())
    }

    /// Expand a leading `~` to the user's home directory.
    pub fn expand_path(path: &str) -> String {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        path.replacen('~', &home, 1)
    }

    /// Expanded output folder path.
    pub fn output_dir(&self) -> String {
        Self::expand_path(&self.output_folder)
    }

    /// Optional cookies file path for yt-dlp (None if blank).
    pub fn cookies_opt(&self) -> Option<String> {
        let s = self.ytdlp_cookies.trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(Self::expand_path(&s))
        }
    }

    /// `--limit-rate` arg value, or None if rate_limit == "off".
    pub fn rate_limit_arg(&self) -> Option<&str> {
        if self.rate_limit == "off" {
            None
        } else {
            Some(&self.rate_limit)
        }
    }
}

fn config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".config").join("dora").join("settings.json")
}
