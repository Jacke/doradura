//! Application state for the dora TUI.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use chrono::Local;
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::settings::DoraSettings;
use crate::theme::{palette, ThemeColors};

/// Re-export LogoScheme so that `ui/logo.rs` and other modules can import from `crate::app`.
pub use crate::theme::LogoScheme;
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

/// Sort order for the history panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HistorySortMode {
    /// Newest first (default).
    #[default]
    DateDesc,
    /// Oldest first.
    DateAsc,
    /// Largest file first.
    SizeDesc,
    /// Alphabetical by title.
    TitleAsc,
}

impl HistorySortMode {
    pub fn next(self) -> Self {
        match self {
            Self::DateDesc => Self::DateAsc,
            Self::DateAsc => Self::SizeDesc,
            Self::SizeDesc => Self::TitleAsc,
            Self::TitleAsc => Self::DateDesc,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::DateDesc => "Date ↓",
            Self::DateAsc => "Date ↑",
            Self::SizeDesc => "Size ↓",
            Self::TitleAsc => "Title A-Z",
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
    /// Close the preview popup.
    PreviewClose,
    /// Select (focus) a settings item by index.
    SettingsSelectItem(usize),
    /// Cycle a settings item left (←).
    SettingsCycleLeft(usize),
    /// Cycle a settings item right (→).
    SettingsCycleRight(usize),
    /// Click on the logo — cycle colour scheme + trigger burst animation.
    LogoClick,
    /// Open the history detail popup. Contains the absolute position in the filtered list.
    HistoryOpenPopup(usize),
    /// Click on the ASCII art panel of the history popup — reveal the file.
    HistoryReveal(String),
    /// Click on the artist name in lyrics to see their other songs. (ID, Name).
    ArtistClick(Option<u64>, String),
    /// Click on a song in the artist's song list to fetch its lyrics.
    ArtistSongClick(String, String),
    /// Open Genius API clients page in browser.
    GetGeniusToken,
    /// Load more results in lyrics grid.
    LyricsLoadMore,
    /// Toggle subtitle menu visibility in preview popup.
    PreviewToggleSubsMenu,
    /// Toggle subtitle burn on/off.
    PreviewToggleSubsEnabled,
    /// Select subtitle language by index.
    PreviewSubsLang(usize),
}

/// State of a single download slot.
#[derive(Debug, Clone)]
pub enum SlotState {
    Pending,
    Fetching,
    Downloading {
        percent: u8,
        speed_mbs: f64,
        eta_secs: u64,
    },
    /// 1-second colour burst animation played when a download completes.
    Celebrating {
        path: String,
        started: Instant,
    },
    Done {
        path: String,
    },
    Failed {
        reason: String,
    },
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
    pub thumbnail_url: Option<String>,
    pub format: DownloadFormat,
    pub state: SlotState,
    #[allow(dead_code)]
    pub started: Instant,
    /// True once `spawn_download` has been called for this slot.
    pub task_spawned: bool,
    /// Ring buffer of the last 20 download speed samples (MB/s).
    pub speed_history: VecDeque<f64>,
    /// Handle to abort the background download task (set when task is spawned).
    pub cancel: Option<tokio::task::AbortHandle>,
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
    /// Original source URL.
    #[serde(default)]
    pub url: String,
    /// Thumbnail URL for image preview.
    #[serde(default)]
    pub thumbnail_url: Option<String>,
}

/// A lyrics search result.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct LyricsResult {
    pub artist: String,
    pub artist_id: Option<u64>,
    pub title: String,
    pub album: Option<String>,
    pub release_date: Option<String>,
    pub thumbnail_url: Option<String>,
    pub lyrics: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LyricsViewMode {
    #[default]
    Lyrics,
    ArtistSongs,
}

/// A single supernova particle ejected when a download completes.
pub struct Particle {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub ch: char,
    pub color: Color,
    pub age: f32,
    pub max_age: f32,
}

/// A single TUI toast notification.
#[derive(Debug, Clone)]
pub struct Toast {
    pub message: String,
    pub kind: ToastKind,
    pub added_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

/// Central application state.
pub struct App {
    pub active_tab: Tab,
    pub slots: Vec<DownloadSlot>,
    pub history: Vec<HistoryEntry>,
    /// Scroll offset (first visible row).
    pub history_scroll: u16,
    /// Currently highlighted entry index (absolute position in filtered list).
    pub history_index: usize,
    /// Display-index (0 = newest) of the history entry currently shown in the detail popup.
    pub history_popup: Option<usize>,
    pub url_input: String,

    /// Live notifications (toasts) displayed in the corner.
    pub toasts: Vec<Toast>,

    // ── History search / sort ──────────────────────────────────────────────────
    /// When true, keyboard input goes to the history filter bar.
    pub history_search_mode: bool,
    /// Current filter string (live-filtered in history panel).
    pub history_filter: String,
    /// Current sort order for the history panel.
    pub history_sort: HistorySortMode,
    /// Indices of selected history entries (multi-selection).
    pub history_selected: std::collections::HashSet<usize>,
    /// Cached indices of history entries matching the current filter.
    pub history_filtered_indices: Vec<usize>,

    // ── Lyrics tab ────────────────────────────────────────────────────────────
    pub lyrics_query: String,
    pub lyrics_result: Option<LyricsResult>,
    pub lyrics_loading: bool,
    pub lyrics_scroll: u16,
    pub lyrics_view_mode: LyricsViewMode,
    pub artist_songs: Vec<doracore::lyrics::ArtistSong>,
    pub artist_songs_cursor: usize,
    pub artist_songs_page: u32,
    pub last_artist_id: Option<u64>,
    pub last_lyrics_query: String,

    // ── Overlays ──────────────────────────────────────────────────────────────
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
    /// High-quality image protocol (Kitty/Sixel) cached for rendering.
    pub preview_image_protocol: Option<ratatui_image::protocol::Protocol>,
    // ── Subtitle sub-menu in preview ───────────────────────────────────────────
    /// Whether the subtitle sub-menu is currently showing.
    pub preview_subs_menu: bool,
    /// Whether burned subtitles are enabled for current preview.
    pub preview_subs_enabled: bool,
    /// Selected subtitle language index in the available list.
    pub preview_subs_lang_cursor: usize,
    /// Custom subtitle language code (typed by user).
    pub preview_subs_custom_lang: Option<String>,
    /// Whether user is typing a custom language code.
    pub preview_subs_editing: bool,
    /// Text buffer for custom language input.
    pub preview_subs_edit_buf: String,

    /// True when the run loop should spawn the video-info fetch task.
    pub preview_fetch_needed: bool,
    /// URL waiting for debounce before the preview fetch is dispatched.
    pub preview_pending_url: Option<String>,
    /// When `preview_pending_url` was last set (used for 300ms debounce).
    pub preview_debounce: Instant,
    /// Cache of already-fetched VideoInfo (keyed by normalised URL).
    pub preview_cache: HashMap<String, VideoInfo>,
    /// Clickable regions rebuilt each frame. Used by the mouse handler.
    pub click_map: Vec<(Rect, ClickTarget)>,

    /// Image protocol picker (Kitty, Sixel, etc.).
    pub image_picker: Option<ratatui_image::picker::Picker>,

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
    /// Cursor blink state — toggled every 500 ms by the tick handler.
    pub blink_on: bool,
    /// When the cursor blink was last toggled.
    pub last_blink: Instant,

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

    // ── Session stats ─────────────────────────────────────────────────────────
    /// When the app was started (for uptime display in the stats footer).
    pub session_start: Instant,

    // ── Active theme ──────────────────────────────────────────────────────────
    /// Current Catppuccin colour palette.  Cycled with `[T]`.
    pub theme: ThemeColors,

    // ── Supernova particle system ─────────────────────────────────────────────
    /// Particles spawned when a download completes its 1-second celebration.
    pub particles: Vec<Particle>,
    /// Terminal rects of rendered download slots (populated by queue renderer each
    /// frame; used by `tick()` to position the supernova burst).
    pub slot_screen_rects: HashMap<usize, Rect>,

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
        let now = Instant::now();
        let theme = palette(settings.theme_flavour);
        let logo_scheme = settings.logo_scheme; // Copy before settings is moved into Self
        let mut app = Self {
            active_tab: Tab::Downloads,
            slots: Vec::new(),
            history: history_load(),
            history_scroll: 0,
            history_index: 0,
            history_popup: None,
            url_input: String::new(),
            toasts: Vec::new(),
            history_search_mode: false,
            history_filter: String::new(),
            history_sort: HistorySortMode::default(),
            history_selected: std::collections::HashSet::new(),
            history_filtered_indices: Vec::new(),
            lyrics_query: String::new(),
            lyrics_result: None,
            lyrics_loading: false,
            lyrics_scroll: 0,
            lyrics_view_mode: LyricsViewMode::Lyrics,
            artist_songs: Vec::new(),
            artist_songs_cursor: 0,
            artist_songs_page: 1,
            last_artist_id: None,
            last_lyrics_query: String::new(),
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
            preview_format: DownloadFormat::Mp4,
            preview_quality_cursor: 0,
            preview_thumbnail: None,
            preview_image_protocol: None,
            preview_subs_menu: false,
            preview_subs_enabled: false,
            preview_subs_lang_cursor: 0,
            preview_subs_custom_lang: None,
            preview_subs_editing: false,
            preview_subs_edit_buf: String::new(),
            preview_fetch_needed: false,
            preview_pending_url: None,
            preview_debounce: now,
            preview_cache: HashMap::new(),
            click_map: Vec::new(),
            image_picker: ratatui_image::picker::Picker::from_query_stdio().ok(),
            settings,
            settings_cursor: 0,
            settings_editing: false,
            settings_edit_buf: String::new(),
            settings_file_picker_field: None,
            logo_frame: 0,
            spinner_frame: 0,
            last_tick: now,
            blink_on: true,
            last_blink: now,
            logo_scheme,
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
            session_start: now,
            theme,
            particles: Vec::new(),
            slot_screen_rects: HashMap::new(),
            next_slot_id: 0,
        };
        app.update_history_filter();
        app
    }

    /// Create an app pre-populated with fake data for visual testing (`--demo`).
    pub fn new_demo() -> Self {
        let mut app = Self::new();
        app.demo_mode = true;
        app.image_picker = ratatui_image::picker::Picker::from_query_stdio().ok();
        app.history.clear(); // Replace loaded history with demo data below

        let now = Local::now();
        app.slots = vec![
            DownloadSlot {
                id: 0,
                url: "https://youtu.be/dQw4w9WgXcQ".to_string(),
                title: Some("Never Gonna Give You Up".to_string()),
                artist: Some("Rick Astley".to_string()),
                thumbnail_url: None,
                format: DownloadFormat::Mp3,
                state: SlotState::Downloading {
                    percent: 42,
                    speed_mbs: 8.3,
                    eta_secs: 18,
                },
                started: Instant::now(),
                task_spawned: true,
                speed_history: {
                    let mut dq = VecDeque::new();
                    for i in 0..20usize {
                        let v = 8.0 + (i as f64 * 0.3).sin() * 4.0;
                        dq.push_back(v.max(0.1));
                    }
                    dq
                },
                cancel: None,
            },
            DownloadSlot {
                id: 1,
                url: "https://youtu.be/L_jWHffIx5E".to_string(),
                title: Some("Smells Like Teen Spirit".to_string()),
                artist: Some("Nirvana".to_string()),
                thumbnail_url: None,
                format: DownloadFormat::Mp4,
                state: SlotState::Fetching,
                started: Instant::now(),
                task_spawned: true,
                speed_history: VecDeque::new(),
                cancel: None,
            },
            DownloadSlot {
                id: 2,
                url: "https://soundcloud.com/artist/some-long-track-name-here".to_string(),
                title: None,
                artist: None,
                thumbnail_url: None,
                format: DownloadFormat::Mp3,
                state: SlotState::Pending,
                started: Instant::now(),
                task_spawned: true, // demo: don't auto-spawn real tasks
                speed_history: VecDeque::new(),
                cancel: None,
            },
            DownloadSlot {
                id: 3,
                url: "https://youtu.be/example".to_string(),
                title: Some("Sweet Child O' Mine".to_string()),
                artist: Some("Guns N' Roses".to_string()),
                thumbnail_url: None,
                format: DownloadFormat::Mp3,
                state: SlotState::Done {
                    path: "~/Downloads/Sweet_Child_O_Mine.mp3".to_string(),
                },
                started: Instant::now(),
                task_spawned: true,
                speed_history: VecDeque::new(),
                cancel: None,
            },
            DownloadSlot {
                id: 4,
                url: "https://youtu.be/bad-video".to_string(),
                title: Some("Unavailable Video".to_string()),
                artist: None,
                thumbnail_url: None,
                format: DownloadFormat::Mp4,
                state: SlotState::Failed {
                    reason: "Video is geo-restricted".to_string(),
                },
                started: Instant::now(),
                task_spawned: true,
                speed_history: VecDeque::new(),
                cancel: None,
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
                thumbnail_url: None,
            },
            HistoryEntry {
                title: "Hotel California".to_string(),
                artist: "Eagles".to_string(),
                format: DownloadFormat::Mp3,
                size_mb: 9.8,
                path: "~/Downloads/Hotel_California.mp3".to_string(),
                finished_at: now - chrono::Duration::seconds(10800),
                url: "https://www.youtube.com/watch?v=BciS5krYL80".to_string(),
                thumbnail_url: None,
            },
            HistoryEntry {
                title: "Comfortably Numb".to_string(),
                artist: "Pink Floyd".to_string(),
                format: DownloadFormat::Mp4,
                size_mb: 156.2,
                path: "~/Downloads/Comfortably_Numb.mp4".to_string(),
                finished_at: now - chrono::Duration::seconds(18000),
                url: "https://www.youtube.com/watch?v=_FrOQC-zEog".to_string(),
                thumbnail_url: None,
            },
            HistoryEntry {
                title: "Stairway to Heaven".to_string(),
                artist: "Led Zeppelin".to_string(),
                format: DownloadFormat::Mp3,
                size_mb: 15.7,
                path: "~/Downloads/Stairway_to_Heaven.mp3".to_string(),
                finished_at: now - chrono::Duration::seconds(86400),
                url: "https://www.youtube.com/watch?v=QkF3oxziUI4".to_string(),
                thumbnail_url: None,
            },
        ];

        app.update_history_filter();
        app
    }

    pub fn tick(&mut self) {
        let dt = self.last_tick.elapsed().as_secs_f32().clamp(0.001, 0.1);
        self.logo_frame = self.logo_frame.wrapping_add(1);
        self.spinner_frame = self.spinner_frame.wrapping_add(1);
        self.last_tick = Instant::now();

        // Wall-clock cursor blink: toggle every 500 ms regardless of FPS.
        if self.last_blink.elapsed() >= Duration::from_millis(500) {
            self.blink_on = !self.blink_on;
            self.last_blink = Instant::now();
        }

        // Feature: TUI Toasts decay
        self.toasts.retain(|t| t.added_at.elapsed() < Duration::from_secs(5));

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

        // Advance Celebrating slots → Done after 1 second; spawn supernova burst.
        let celebrating_done: Vec<(usize, Rect)> = self
            .slots
            .iter()
            .filter_map(|s| {
                if let SlotState::Celebrating { started, .. } = &s.state {
                    if started.elapsed() >= Duration::from_secs(1) {
                        let rect = self.slot_screen_rects.get(&s.id).copied().unwrap_or_default();
                        return Some((s.id, rect));
                    }
                }
                None
            })
            .collect();
        for (id, rect) in celebrating_done {
            if let Some(slot) = self.slot_mut(id) {
                if let SlotState::Celebrating { path, .. } = std::mem::replace(&mut slot.state, SlotState::Pending) {
                    slot.state = SlotState::Done { path };
                }
            }
            // Spawn particles only when we have a valid screen rect.
            if rect.width > 0 && rect.height > 0 {
                let cx = rect.x as f32 + rect.width as f32 / 2.0;
                let cy = rect.y as f32 + rect.height as f32 / 2.0;
                let burst = spawn_burst_particles(cx, cy, &self.theme);
                self.particles.extend(burst);
            }
        }

        // Advance particle physics.
        for p in &mut self.particles {
            p.x += p.vx * dt * 30.0; // vx=1 ≈ 30 cells/sec
            p.y += p.vy * dt * 30.0;
            p.vy += 0.06 * dt; // downward gravity
            p.age += dt;
        }
        self.particles.retain(|p| p.age < p.max_age);

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
                    // Push demo speed sample
                    slot.speed_history.push_back(*speed_mbs);
                    if slot.speed_history.len() > 20 {
                        slot.speed_history.pop_front();
                    }
                }
            }
        }
    }

    /// True when the UI has active animations that require a fast redraw (~30 fps).
    /// True when the UI has active animations that require ~30 fps redraws.
    pub fn needs_fast_tick(&self) -> bool {
        let has_download = self
            .slots
            .iter()
            .any(|s| matches!(s.state, SlotState::Downloading { .. } | SlotState::Celebrating { .. }));
        has_download
            || !self.particles.is_empty()
            || self.logo_burst > 0
            || self.lyrics_loading
            || matches!(self.preview_state, PreviewState::Loading)
            || matches!(self.ytdlp_startup, YtdlpStartup::FadingOut { .. })
            || self.demo_mode
    }

    /// True when there is an active text input that needs a blinking cursor (500 ms tick).
    pub fn needs_blink_tick(&self) -> bool {
        use crate::app::Tab;
        !self.url_input.is_empty()
            || self.settings_editing
            || self.show_cookies_input
            || self.history_search_mode
            || self.preview_subs_editing
            || (self.active_tab == Tab::Lyrics && !self.lyrics_query.is_empty())
    }

    pub fn add_toast(&mut self, message: &str, kind: ToastKind) {
        self.toasts.push(Toast {
            message: message.to_string(),
            kind,
            added_at: Instant::now(),
        });
        // Limit to 5 active toasts to avoid clutter
        if self.toasts.len() > 5 {
            self.toasts.remove(0);
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
            thumbnail_url: None,
            format,
            state: SlotState::Pending,
            started: Instant::now(),
            task_spawned: false,
            speed_history: VecDeque::new(),
            cancel: None,
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

    pub fn update_history_filter(&mut self) {
        let filter = self.history_filter.to_lowercase();
        let mut indices: Vec<usize> = if filter.is_empty() {
            (0..self.history.len()).collect()
        } else {
            self.history
                .iter()
                .rev()
                .enumerate()
                .filter_map(|(display_idx, e)| {
                    let haystack = format!("{} {}", e.title, e.artist).to_lowercase();
                    if fuzzy_match(&filter, &haystack) {
                        Some(display_idx)
                    } else {
                        None
                    }
                })
                .collect()
        };

        match self.history_sort {
            HistorySortMode::DateDesc => {}
            HistorySortMode::DateAsc => indices.reverse(),
            HistorySortMode::SizeDesc => {
                let history = &self.history;
                indices.sort_by(|&a, &b| {
                    let ea = history.iter().rev().nth(a).map_or(0.0, |e| e.size_mb);
                    let eb = history.iter().rev().nth(b).map_or(0.0, |e| e.size_mb);
                    eb.partial_cmp(&ea).unwrap_or(std::cmp::Ordering::Equal)
                });
            }
            HistorySortMode::TitleAsc => {
                let history = &self.history;
                indices.sort_by(|&a, &b| {
                    let ta = history.iter().rev().nth(a).map_or("", |e| e.title.as_str());
                    let tb = history.iter().rev().nth(b).map_or("", |e| e.title.as_str());
                    ta.to_lowercase().cmp(&tb.to_lowercase())
                });
            }
        }
        self.history_filtered_indices = indices;

        // Clamp index to new bounds
        let max = self.history_filtered_indices.len().saturating_sub(1);
        if self.history_index > max {
            self.history_index = max;
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

fn fuzzy_match(query: &str, haystack: &str) -> bool {
    let mut haystack_chars = haystack.chars();
    for q_char in query.chars() {
        match haystack_chars.find(|&h_char| h_char == q_char) {
            Some(_) => (),
            None => return false,
        }
    }
    true
}

// ── Supernova particle burst ──────────────────────────────────────────────────

/// Spawn a sparkle burst of 8 particles from (cx, cy) on mouse click.
/// Short lifetime (0.2s), radius ~2 cells — subtle but satisfying.
pub fn spawn_ripple_particles(cx: f32, cy: f32, theme: &ThemeColors) -> Vec<Particle> {
    const CHARS: &[char] = &['✦', '*', '·', '+', '✦', '·', '*', '+'];
    let colors = [theme.lavender, theme.mauve, theme.peach, theme.yellow];
    let mut out = Vec::with_capacity(8);
    for i in 0..8usize {
        let angle = (i as f32 / 8.0) * std::f32::consts::TAU;
        // speed chosen so particles travel ~2 cells in 0.2s: d = v * dt * 30 * t
        let speed = 0.28 + (i % 3) as f32 * 0.04;
        out.push(Particle {
            x: cx,
            y: cy,
            vx: angle.cos() * speed,
            vy: angle.sin() * speed * 0.5, // narrower vertically (char aspect ratio)
            ch: CHARS[i],
            color: colors[i % colors.len()],
            age: 0.0,
            max_age: 0.20,
        });
    }
    out
}

/// Spawn a burst of 24 particles from terminal cell (cx, cy).
fn spawn_burst_particles(cx: f32, cy: f32, theme: &ThemeColors) -> Vec<Particle> {
    const CHARS: &[char] = &['*', '·', '°', '★', '+', '×', '•', '◦', '✧', '⋆', '✦', '✨'];
    let colors = [
        theme.lavender,
        theme.green,
        theme.yellow,
        theme.peach,
        theme.mauve,
        theme.teal,
        theme.blue,
    ];
    let mut out = Vec::with_capacity(24);
    for i in 0..24usize {
        let angle = (i as f32 / 24.0) * std::f32::consts::TAU;
        let speed = 0.15 + (i % 4) as f32 * 0.08;
        out.push(Particle {
            x: cx,
            y: cy,
            vx: angle.cos() * speed,
            vy: angle.sin() * speed * 0.45, // narrower vertical spread (char aspect)
            ch: CHARS[i % CHARS.len()],
            color: colors[i % colors.len()],
            age: 0.0,
            max_age: 1.2 + (i % 5) as f32 * 0.25, // 1.2 – 2.2 seconds
        });
    }
    out
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
