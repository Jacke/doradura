//! Application state for the dora TUI.

use std::time::Instant;

use chrono::Local;
use ratatui::layout::Rect;

use crate::settings::DoraSettings;
use crate::video_info::{ThumbnailArt, VideoInfo};

/// Which tab is currently active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Downloads = 0,
    Lyrics = 1,
    Settings = 2,
}

impl Tab {
    #[allow(dead_code)]
    pub fn from_index(i: usize) -> Option<Self> {
        match i {
            0 => Some(Tab::Downloads),
            1 => Some(Tab::Lyrics),
            2 => Some(Tab::Settings),
            _ => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Tab::Downloads => "[1] ⬇  Downloads",
            Tab::Lyrics => "[2] 🎵 Lyrics",
            Tab::Settings => "[3] ⚙  Settings",
        }
    }

    pub fn index(&self) -> usize {
        *self as usize
    }
}

/// State of the video preview popup.
#[derive(Debug, Clone, Default)]
pub enum PreviewState {
    #[default]
    Hidden,
    /// Info is being fetched in the background.
    Loading,
    /// Info arrived — show metadata + quality selector.
    Ready { info: VideoInfo },
    /// Fetch failed — show error with option to download anyway.
    Failed(String),
}

impl PreviewState {
    pub fn is_visible(&self) -> bool {
        !matches!(self, PreviewState::Hidden)
    }
}

/// State of the yt-dlp startup check/update.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum YtdlpStartup {
    /// Not showing any popup (check done / skipped).
    #[default]
    Done,
    /// Binary not found — show install-or-quit dialog.
    Missing,
    /// Binary found, running `yt-dlp -U` in the background.
    Updating { msg: String },
    /// Update finished — popup fades out over `ticks` frames (0 → Done).
    FadingOut { ticks: u8 },
}

/// Logo colour scheme, cycled on logo click.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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

/// Clickable regions registered by the renderer each frame.
#[derive(Debug, Clone)]
pub enum ClickTarget {
    /// Switch to a specific tab.
    SwitchTab(Tab),
    /// Open a URL in the system browser.
    OpenInBrowser(String),
    /// Select quality cursor index in the preview popup.
    PreviewQuality(usize),
    /// Toggle MP3 ↔ MP4 in the preview popup.
    PreviewToggleFormat,
    /// Confirm the preview download (same as pressing Enter in the popup).
    PreviewDownload,
    /// Select (focus) a settings item by index.
    SettingsSelectItem(usize),
    /// Cycle a settings item left (←).
    SettingsCycleLeft(usize),
    /// Cycle a settings item right (→).
    SettingsCycleRight(usize),
    /// Click on the logo — cycle colour scheme + trigger burst animation.
    LogoClick,
    /// Open the history detail popup for a specific display-index entry.
    HistoryOpenPopup(usize),
    /// Click on the ASCII art panel of the history popup — reveal the file.
    HistoryReveal(String),
}

/// State of a single download slot.
#[derive(Debug, Clone)]
pub enum SlotState {
    Pending,
    Fetching,
    Downloading { percent: u8, speed_mbs: f64, eta_secs: u64 },
    Done { path: String },
    Failed { reason: String },
}

/// Output format requested by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum DownloadFormat {
    #[default]
    Mp3,
    Mp4,
}

impl DownloadFormat {
    pub fn label(&self) -> &'static str {
        match self {
            DownloadFormat::Mp3 => "MP3",
            DownloadFormat::Mp4 => "MP4",
        }
    }
}

/// One active or queued download.
#[derive(Debug, Clone)]
pub struct DownloadSlot {
    /// Stable identifier (never changes even if other slots are removed).
    pub id: usize,
    pub url: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub format: DownloadFormat,
    pub state: SlotState,
    #[allow(dead_code)]
    pub started: Instant,
    /// True once `spawn_download` has been called for this slot.
    pub task_spawned: bool,
}

/// A completed-download entry kept in history.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HistoryEntry {
    pub title: String,
    pub artist: String,
    pub format: DownloadFormat,
    pub size_mb: f64,
    pub path: String,
    pub finished_at: chrono::DateTime<chrono::Local>,
    /// Original source URL (empty for entries saved before this field was added).
    #[serde(default)]
    pub url: String,
}

/// A lyrics search result.
#[derive(Debug, Clone)]
pub struct LyricsResult {
    pub title: String,
    pub artist: String,
    pub lyrics: String,
}

/// Central application state.
pub struct App {
    pub active_tab: Tab,
    pub slots: Vec<DownloadSlot>,
    pub history: Vec<HistoryEntry>,
    pub history_scroll: u16,
    /// Display-index (0 = newest) of the history entry currently shown in the detail popup.
    pub history_popup: Option<usize>,
    pub url_input: String,

    // ── Lyrics tab ────────────────────────────────────────────────────────────
    pub lyrics_query: String,
    pub lyrics_result: Option<LyricsResult>,
    pub lyrics_loading: bool,
    pub lyrics_scroll: u16,

    // ── Overlays ──────────────────────────────────────────────────────────────
    pub error_popup: Option<String>,
    pub help_visible: bool,

    /// When true, a cookies-file input popup is displayed.
    pub show_cookies_input: bool,
    /// Text being typed in the cookies popup.
    pub cookies_input: String,
    /// Resolved cookies file path (passed to yt-dlp with --cookies).
    pub cookies_file: Option<String>,

    /// When Some(path), shows the "reveal file" path popup (non-macOS).
    pub reveal_popup: Option<String>,

    // ── Video preview popup ────────────────────────────────────────────────────
    pub preview_state: PreviewState,
    /// URL currently being previewed.
    pub preview_url: String,
    /// Format selected in preview (may differ from format_selected while popup is open).
    pub preview_format: DownloadFormat,
    /// Selected quality index into VideoInfo::available_heights (+ "best" at end).
    pub preview_quality_cursor: usize,
    /// Thumbnail ASCII art (loaded asynchronously after info arrives).
    pub preview_thumbnail: Option<ThumbnailArt>,
    /// True when the run loop should spawn the video-info fetch task.
    pub preview_fetch_needed: bool,
    /// Clickable regions rebuilt each frame. Used by the mouse handler.
    pub click_map: Vec<(Rect, ClickTarget)>,

    // ── Settings tab ──────────────────────────────────────────────────────────
    pub settings: DoraSettings,
    /// Index of the currently selected settings item (0-10).
    pub settings_cursor: usize,
    /// True when a text item is in edit mode.
    pub settings_editing: bool,
    /// Buffer for in-progress text edits.
    pub settings_edit_buf: String,
    /// If Some(idx), the next file-picker result goes to settings field `idx`.
    pub settings_file_picker_field: Option<usize>,

    // ── Animation counters ────────────────────────────────────────────────────
    pub logo_frame: u16,
    pub spinner_frame: u8,
    pub last_tick: Instant,

    // ── Logo easter egg ───────────────────────────────────────────────────────
    /// Active colour scheme for the logo.
    pub logo_scheme: LogoScheme,
    /// Burst animation counter (>0 while animating after a click; counts down each tick).
    pub logo_burst: u8,

    pub demo_mode: bool,

    /// Whether the yt-dlp binary is available on PATH at startup.
    pub ytdlp_available: bool,

    /// State of the yt-dlp startup check popup.
    pub ytdlp_startup: YtdlpStartup,

    // ── Internal ──────────────────────────────────────────────────────────────
    next_slot_id: usize,
}

impl App {
    pub fn new() -> Self {
        let settings = DoraSettings::load();
        let ytdlp_available = std::process::Command::new(&settings.ytdlp_bin)
            .arg("--version")
            .output()
            .is_ok();
        Self {
            active_tab: Tab::Downloads,
            slots: Vec::new(),
            history: history_load(),
            history_scroll: 0,
            history_popup: None,
            url_input: String::new(),
            lyrics_query: String::new(),
            lyrics_result: None,
            lyrics_loading: false,
            lyrics_scroll: 0,
            error_popup: None,
            help_visible: false,
            show_cookies_input: false,
            cookies_input: String::new(),
            cookies_file: if settings.ytdlp_cookies.is_empty() {
                None
            } else {
                Some(settings.ytdlp_cookies.clone())
            },
            reveal_popup: None,
            preview_state: PreviewState::Hidden,
            preview_url: String::new(),
            preview_format: DownloadFormat::Mp3,
            preview_quality_cursor: 0,
            preview_thumbnail: None,
            preview_fetch_needed: false,
            click_map: Vec::new(),
            settings,
            settings_cursor: 0,
            settings_editing: false,
            settings_edit_buf: String::new(),
            settings_file_picker_field: None,
            logo_frame: 0,
            spinner_frame: 0,
            last_tick: Instant::now(),
            logo_scheme: LogoScheme::default(),
            logo_burst: 0,
            demo_mode: false,
            ytdlp_available,
            ytdlp_startup: if ytdlp_available {
                YtdlpStartup::Updating {
                    msg: "Checking for updates…".to_string(),
                }
            } else {
                YtdlpStartup::Missing
            },
            next_slot_id: 0,
        }
    }

    /// Create an app pre-populated with fake data for visual testing (`--demo`).
    pub fn new_demo() -> Self {
        let mut app = Self::new();
        app.demo_mode = true;
        app.history.clear(); // Replace loaded history with demo data below

        let now = Local::now();
        app.slots = vec![
            DownloadSlot {
                id: 0,
                url: "https://youtu.be/dQw4w9WgXcQ".to_string(),
                title: Some("Never Gonna Give You Up".to_string()),
                artist: Some("Rick Astley".to_string()),
                format: DownloadFormat::Mp3,
                state: SlotState::Downloading {
                    percent: 42,
                    speed_mbs: 8.3,
                    eta_secs: 18,
                },
                started: Instant::now(),
                task_spawned: true,
            },
            DownloadSlot {
                id: 1,
                url: "https://youtu.be/L_jWHffIx5E".to_string(),
                title: Some("Smells Like Teen Spirit".to_string()),
                artist: Some("Nirvana".to_string()),
                format: DownloadFormat::Mp4,
                state: SlotState::Fetching,
                started: Instant::now(),
                task_spawned: true,
            },
            DownloadSlot {
                id: 2,
                url: "https://soundcloud.com/artist/some-long-track-name-here".to_string(),
                title: None,
                artist: None,
                format: DownloadFormat::Mp3,
                state: SlotState::Pending,
                started: Instant::now(),
                task_spawned: true, // demo: don't auto-spawn real tasks
            },
            DownloadSlot {
                id: 3,
                url: "https://youtu.be/example".to_string(),
                title: Some("Sweet Child O' Mine".to_string()),
                artist: Some("Guns N' Roses".to_string()),
                format: DownloadFormat::Mp3,
                state: SlotState::Done {
                    path: "~/Downloads/Sweet_Child_O_Mine.mp3".to_string(),
                },
                started: Instant::now(),
                task_spawned: true,
            },
            DownloadSlot {
                id: 4,
                url: "https://youtu.be/bad-video".to_string(),
                title: Some("Unavailable Video".to_string()),
                artist: None,
                format: DownloadFormat::Mp4,
                state: SlotState::Failed {
                    reason: "Video is geo-restricted".to_string(),
                },
                started: Instant::now(),
                task_spawned: true,
            },
        ];
        app.next_slot_id = 5;

        app.history = vec![
            HistoryEntry {
                title: "Bohemian Rhapsody".to_string(),
                artist: "Queen".to_string(),
                format: DownloadFormat::Mp3,
                size_mb: 12.4,
                path: "~/Downloads/Bohemian_Rhapsody.mp3".to_string(),
                finished_at: now - chrono::Duration::seconds(3600),
                url: "https://www.youtube.com/watch?v=fJ9rUzIMcZQ".to_string(),
            },
            HistoryEntry {
                title: "Hotel California".to_string(),
                artist: "Eagles".to_string(),
                format: DownloadFormat::Mp3,
                size_mb: 9.8,
                path: "~/Downloads/Hotel_California.mp3".to_string(),
                finished_at: now - chrono::Duration::seconds(10800),
                url: "https://www.youtube.com/watch?v=BciS5krYL80".to_string(),
            },
            HistoryEntry {
                title: "Comfortably Numb".to_string(),
                artist: "Pink Floyd".to_string(),
                format: DownloadFormat::Mp4,
                size_mb: 156.2,
                path: "~/Downloads/Comfortably_Numb.mp4".to_string(),
                finished_at: now - chrono::Duration::seconds(18000),
                url: "https://www.youtube.com/watch?v=_FrOQC-zEog".to_string(),
            },
            HistoryEntry {
                title: "Stairway to Heaven".to_string(),
                artist: "Led Zeppelin".to_string(),
                format: DownloadFormat::Mp3,
                size_mb: 15.7,
                path: "~/Downloads/Stairway_to_Heaven.mp3".to_string(),
                finished_at: now - chrono::Duration::seconds(86400),
                url: "https://www.youtube.com/watch?v=QkF3oxziUI4".to_string(),
            },
        ];

        app
    }

    pub fn tick(&mut self) {
        self.logo_frame = self.logo_frame.wrapping_add(1);
        self.spinner_frame = self.spinner_frame.wrapping_add(1);
        self.last_tick = Instant::now();

        // Decay burst animation counter.
        if self.logo_burst > 0 {
            self.logo_burst -= 1;
        }

        // Tick down the yt-dlp update fade-out animation.
        if let YtdlpStartup::FadingOut { ref mut ticks } = self.ytdlp_startup {
            if *ticks == 0 {
                self.ytdlp_startup = YtdlpStartup::Done;
            } else {
                *ticks -= 1;
            }
        }

        // In demo mode, animate the downloading slot
        if self.demo_mode && self.spinner_frame.is_multiple_of(12) {
            for slot in &mut self.slots {
                if let SlotState::Downloading {
                    ref mut percent,
                    ref mut speed_mbs,
                    ref mut eta_secs,
                } = slot.state
                {
                    *percent = percent.wrapping_add(1) % 100;
                    let phase = (*percent as f64) * 0.063;
                    *speed_mbs = 8.0 + phase.sin() * 6.0;
                    *eta_secs = (100u64.saturating_sub(*percent as u64)).saturating_mul(12) / 60;
                }
            }
        }
    }

    /// Add a new download slot. Returns the slot's stable ID.
    pub fn add_download(&mut self, url: String, format: DownloadFormat) -> usize {
        let id = self.next_slot_id;
        self.next_slot_id += 1;
        self.slots.push(DownloadSlot {
            id,
            url,
            title: None,
            artist: None,
            format,
            state: SlotState::Pending,
            started: Instant::now(),
            task_spawned: false,
        });
        id
    }

    /// Find a slot by its stable ID and apply a closure to it.
    pub fn slot_mut(&mut self, id: usize) -> Option<&mut DownloadSlot> {
        self.slots.iter_mut().find(|s| s.id == id)
    }

    /// Clamp history_scroll to valid range.
    pub fn clamp_history_scroll(&mut self) {
        let max = self.history.len().saturating_sub(1) as u16;
        self.history_scroll = self.history_scroll.min(max);
    }

    /// Push a history entry and persist to disk.
    pub fn push_history(&mut self, entry: HistoryEntry) {
        self.history.push(entry);
        history_save(&self.history);
    }

    /// Persist the current history to disk.
    pub fn history_save(&self) {
        history_save(&self.history);
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

// ── History persistence ───────────────────────────────────────────────────────

fn history_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    std::path::PathBuf::from(home)
        .join(".config")
        .join("dora")
        .join("history.json")
}

/// Load history from disk; returns empty vec on error.
pub fn history_load() -> Vec<HistoryEntry> {
    let path = history_path();
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(entries) = serde_json::from_str::<Vec<HistoryEntry>>(&content) {
            return entries;
        }
    }
    Vec::new()
}

/// Overwrite the history file (best-effort; silently ignores errors).
pub fn history_save(entries: &[HistoryEntry]) {
    let path = history_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(entries) {
        let _ = std::fs::write(path, json);
    }
}
