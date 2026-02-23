//! dora — beautiful TUI media downloader
//!
//! # Usage
//!
//! ```text
//! dora [--demo]
//! ```
//!
//! # Controls
//!
//! | Key          | Action                           |
//! |--------------|----------------------------------|
//! | `1` / `2` / `3` | Switch tabs                |
//! | `Tab`        | Toggle MP3 / MP4 format          |
//! | `Enter`      | Start download / search lyrics   |
//! | `c`          | Set cookies file (Queue tab)     |
//! | `d`          | Remove last finished/failed slot |
//! | `?`          | Open help overlay                |
//! | `Esc`        | Close popup / clear input        |
//! | `Ctrl+C`     | Quit                             |

use std::io;
use std::time::Duration;

use simplelog::{Config as LogConfig, LevelFilter, WriteLogger};

use crossterm::{
    event::KeyCode,
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::prelude::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;

mod app;
mod download_runner;
mod events;
mod settings;
mod theme;
mod ui;
mod video_info;

use app::{App, DownloadFormat, HistoryEntry, LyricsResult, PreviewState, SlotState, Tab};
use download_runner::{spawn_download, SlotEvent};
use events::{next_event, InputEvent};
use settings::DoraSettings;
use video_info::{fetch_thumbnail_art, fetch_video_info, PreviewResult};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let demo = args.iter().any(|a| a == "--demo");
    let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");

    if verbose {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let log_dir = std::path::PathBuf::from(&home).join(".config").join("dora");
        let _ = std::fs::create_dir_all(&log_dir);
        let log_path = log_dir.join("dora.log");
        if let Ok(file) = std::fs::File::create(&log_path) {
            let _ = WriteLogger::init(LevelFilter::Debug, LogConfig::default(), file);
            log::info!("dora started (verbose mode) — log: {}", log_path.display());
        }
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, crossterm::event::EnableMouseCapture)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut app = if demo { App::new_demo() } else { App::new() };
    let tick_rate = Duration::from_millis(16); // ~60 fps

    // Channel: background download tasks → main loop
    let (dl_tx, dl_rx) = mpsc::channel::<(usize, SlotEvent)>(256);
    // Channel: background lyrics fetch → main loop (None = not found)
    let (lyrics_tx, lyrics_rx) = mpsc::channel::<Option<LyricsResult>>(4);
    // Channel: native file-picker result → main loop
    let (picker_tx, picker_rx) = mpsc::channel::<String>(4);
    // Channel: background video-info fetch → main loop
    let (preview_tx, preview_rx) = mpsc::channel::<PreviewResult>(4);
    // Channel: thumbnail art (arrives separately, after info) → main loop
    let (thumb_tx, thumb_rx) = mpsc::channel::<video_info::ThumbnailArt>(4);

    let result = run_loop(
        &mut terminal,
        &mut app,
        tick_rate,
        dl_tx,
        dl_rx,
        lyrics_tx,
        lyrics_rx,
        picker_tx,
        picker_rx,
        preview_tx,
        preview_rx,
        thumb_tx,
        thumb_rx,
    )
    .await;

    // Always restore terminal, even on error
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        crossterm::event::DisableMouseCapture
    );
    let _ = terminal.show_cursor();

    result
}

#[allow(clippy::too_many_arguments)]
async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    tick_rate: Duration,
    dl_tx: mpsc::Sender<(usize, SlotEvent)>,
    mut dl_rx: mpsc::Receiver<(usize, SlotEvent)>,
    lyrics_tx: mpsc::Sender<Option<LyricsResult>>,
    mut lyrics_rx: mpsc::Receiver<Option<LyricsResult>>,
    picker_tx: mpsc::Sender<String>,
    mut picker_rx: mpsc::Receiver<String>,
    preview_tx: mpsc::Sender<PreviewResult>,
    mut preview_rx: mpsc::Receiver<PreviewResult>,
    thumb_tx: mpsc::Sender<video_info::ThumbnailArt>,
    mut thumb_rx: mpsc::Receiver<video_info::ThumbnailArt>,
) -> anyhow::Result<()> {
    loop {
        // ── Draw ─────────────────────────────────────────────────────────────
        terminal.draw(|f| ui::render(f, app))?;

        // ── Drain download slot events (non-blocking) ─────────────────────────
        while let Ok((slot_id, event)) = dl_rx.try_recv() {
            handle_slot_event(app, slot_id, event);
        }

        // ── Drain lyrics events (non-blocking) ────────────────────────────────
        while let Ok(result) = lyrics_rx.try_recv() {
            app.lyrics_loading = false;
            app.lyrics_result = result;
        }

        // ── Drain video preview results ───────────────────────────────────────
        while let Ok(result) = preview_rx.try_recv() {
            match result {
                Ok((info, _thumb)) => {
                    // Thumbnail arrives separately on thumb_rx — show ready state first
                    app.preview_state = PreviewState::Ready { info };
                }
                Err(msg) => {
                    app.preview_state = PreviewState::Failed(msg);
                }
            }
        }

        // ── Drain thumbnail art results ───────────────────────────────────────
        while let Ok(art) = thumb_rx.try_recv() {
            app.preview_thumbnail = Some(art);
        }

        // ── Drain file-picker results (macOS osascript) ───────────────────────
        while let Ok(path) = picker_rx.try_recv() {
            // Unescape path from Terminal.app drag-and-drop (e.g. "\ " → " ")
            let clean = path.replace("\\ ", " ").trim_matches('"').trim().to_string();
            if !clean.is_empty() {
                if let Some(field_idx) = app.settings_file_picker_field.take() {
                    // Route to settings field (take() clears the field atomically)
                    ui::settings::set_value(app, field_idx, clean);
                } else {
                    app.cookies_input = clean;
                }
            }
        }

        // ── Kick off preview fetch once when preview_fetch_needed is set ──────
        if app.preview_fetch_needed {
            app.preview_fetch_needed = false;
            let url = app.preview_url.clone();
            let ytdlp_bin = if app.settings.ytdlp_bin.trim().is_empty() {
                "yt-dlp".to_string()
            } else {
                app.settings.ytdlp_bin.clone()
            };
            let p_tx = preview_tx.clone();
            let t_tx = thumb_tx.clone();
            tokio::spawn(async move {
                match fetch_video_info(&url, &ytdlp_bin).await {
                    Ok(info) => {
                        let thumb_url = info.thumbnail_url.clone();
                        let _ = p_tx.send(Ok((info, None))).await;
                        if let Some(turl) = thumb_url {
                            if let Some(art) = fetch_thumbnail_art(&turl).await {
                                let _ = t_tx.send(art).await;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = p_tx.send(Err(e)).await;
                    }
                }
            });
        }

        // ── Spawn tasks for newly-queued Pending slots ────────────────────────
        let settings_snap = app.settings.clone();
        let cookies_override = app.cookies_file.clone(); // legacy cookie-popup override
        for slot in &mut app.slots {
            if matches!(slot.state, SlotState::Pending) && !slot.task_spawned {
                slot.task_spawned = true;
                // Legacy cookies popup overrides the settings field
                let mut s = settings_snap.clone();
                if let Some(ref c) = cookies_override {
                    s.ytdlp_cookies = c.clone();
                }
                spawn_download(slot.id, slot.url.clone(), slot.format, s, dl_tx.clone());
            }
        }

        // ── Input event (blocks up to tick_rate) ─────────────────────────────
        match next_event(tick_rate)? {
            InputEvent::Quit => break,

            InputEvent::Tick => {
                app.tick();
            }

            InputEvent::Mouse(mouse) => {
                use crossterm::event::{MouseButton, MouseEventKind};
                match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        let col = mouse.column;
                        let row = mouse.row;
                        let targets: Vec<_> = app.click_map.clone();
                        // Iterate in reverse: later-registered (more specific) targets
                        // take priority over earlier broad ones (e.g. ← → over whole row).
                        for (rect, target) in targets.into_iter().rev() {
                            if col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height
                            {
                                handle_click(app, target, dl_tx.clone());
                                break;
                            }
                        }
                    }
                    MouseEventKind::ScrollDown => match app.active_tab {
                        Tab::History => {
                            let max = app.history.len().saturating_sub(1) as u16;
                            if app.history_scroll < max {
                                app.history_scroll += 1;
                            }
                        }
                        Tab::Lyrics => {
                            app.lyrics_scroll = app.lyrics_scroll.saturating_add(3);
                        }
                        _ => {}
                    },
                    MouseEventKind::ScrollUp => match app.active_tab {
                        Tab::History => {
                            app.history_scroll = app.history_scroll.saturating_sub(1);
                        }
                        Tab::Lyrics => {
                            app.lyrics_scroll = app.lyrics_scroll.saturating_sub(3);
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }

            InputEvent::Key(key) => {
                // ── Esc: universal dismiss (cookies → help → error → reveal → settings edit → clear) ──
                if key.code == KeyCode::Esc {
                    if app.show_cookies_input {
                        app.show_cookies_input = false;
                        app.cookies_input.clear();
                    } else if app.help_visible {
                        app.help_visible = false;
                    } else if app.error_popup.is_some() {
                        app.error_popup = None;
                    } else if app.reveal_popup.is_some() {
                        app.reveal_popup = None;
                    } else if app.preview_state.is_visible() {
                        // Cancel preview — restore URL input
                        let url = std::mem::take(&mut app.preview_url);
                        app.url_input = url;
                        app.preview_state = PreviewState::Hidden;
                        app.preview_thumbnail = None;
                    } else if app.settings_editing {
                        app.settings_editing = false;
                        app.settings_edit_buf.clear();
                    } else {
                        match app.active_tab {
                            Tab::Queue => app.url_input.clear(),
                            Tab::Lyrics => app.lyrics_query.clear(),
                            Tab::History => {}
                            Tab::Settings => {}
                        }
                    }
                    continue;
                }

                // ── Cookies input popup intercepts all keys ───────────────────
                if app.show_cookies_input {
                    match key.code {
                        KeyCode::Enter => {
                            // Trim and unescape drag-and-drop paths from Terminal.app
                            let raw = app.cookies_input.trim().to_string();
                            let path = raw.replace("\\ ", " ").trim_matches('"').trim().to_string();
                            if path.is_empty() {
                                app.cookies_file = None;
                                app.settings.ytdlp_cookies = String::new();
                            } else {
                                app.cookies_file = Some(path.clone());
                                app.settings.ytdlp_cookies = path;
                            }
                            let _ = app.settings.save(); // persist across restarts
                            app.show_cookies_input = false;
                        }
                        // [Del] — clear the stored cookies file entirely
                        KeyCode::Delete => {
                            app.cookies_input.clear();
                            app.cookies_file = None;
                            app.settings.ytdlp_cookies = String::new();
                            let _ = app.settings.save();
                        }
                        KeyCode::Backspace => {
                            app.cookies_input.pop();
                        }
                        // [o] — open native macOS file picker
                        KeyCode::Char('o') => {
                            let tx = picker_tx.clone();
                            tokio::spawn(async move {
                                open_file_picker(tx).await;
                            });
                        }
                        KeyCode::Char(c) => {
                            app.cookies_input.push(c);
                        }
                        _ => {}
                    }
                    continue;
                }

                // ── Any key dismisses reveal popup ────────────────────────────
                if app.reveal_popup.is_some() {
                    app.reveal_popup = None;
                    continue;
                }

                // ── Preview popup intercepts all keys ─────────────────────────
                if app.preview_state.is_visible() {
                    handle_preview_key(app, key, dl_tx.clone());
                    continue;
                }

                // ── Any key dismisses error popup ─────────────────────────────
                if app.error_popup.is_some() {
                    app.error_popup = None;
                    continue;
                }

                // ── Any key closes help overlay ───────────────────────────────
                if app.help_visible {
                    app.help_visible = false;
                    continue;
                }

                // ── '?' opens help (only when not typing in a text input) ────
                if key.code == KeyCode::Char('?') {
                    let typing = (app.active_tab == Tab::Queue && !app.url_input.is_empty())
                        || (app.active_tab == Tab::Lyrics && !app.lyrics_query.is_empty());
                    if !typing {
                        app.help_visible = true;
                        continue;
                    }
                }

                // ── 'c' opens cookies popup (Queue tab, only when not typing) ───
                if app.active_tab == Tab::Queue && app.url_input.is_empty() && key.code == KeyCode::Char('c') {
                    app.show_cookies_input = true;
                    app.cookies_input = app.cookies_file.clone().unwrap_or_default();
                    continue;
                }

                // ── Tab-specific handling ─────────────────────────────────────
                match app.active_tab {
                    Tab::Queue => handle_queue_key(app, key),
                    Tab::History => handle_history_key(app, key),
                    Tab::Lyrics => handle_lyrics_key(app, key, lyrics_tx.clone()),
                    Tab::Settings => handle_settings_key(app, key, picker_tx.clone()),
                }

                // ── Global tab-switching (suppressed while typing in a text input) ──
                let typing_in_input = (app.active_tab == Tab::Queue && !app.url_input.is_empty())
                    || (app.active_tab == Tab::Lyrics && !app.lyrics_query.is_empty());
                if !typing_in_input {
                    match key.code {
                        KeyCode::Char('1') => app.active_tab = Tab::Queue,
                        KeyCode::Char('2') => app.active_tab = Tab::History,
                        KeyCode::Char('3') => app.active_tab = Tab::Lyrics,
                        KeyCode::Char('4') => app.active_tab = Tab::Settings,
                        _ => {}
                    }
                }
            }
        }
    }

    Ok(())
}

// ── Slot event handler ────────────────────────────────────────────────────────

fn handle_slot_event(app: &mut App, slot_id: usize, event: SlotEvent) {
    match event {
        SlotEvent::Fetching => {
            if let Some(slot) = app.slot_mut(slot_id) {
                slot.state = SlotState::Fetching;
            }
        }
        SlotEvent::Metadata { title, artist } => {
            if let Some(slot) = app.slot_mut(slot_id) {
                if title.is_some() {
                    slot.title = title;
                }
                if artist.is_some() {
                    slot.artist = artist;
                }
            }
        }
        SlotEvent::Progress {
            percent,
            speed_mbs,
            eta_secs,
        } => {
            if let Some(slot) = app.slot_mut(slot_id) {
                slot.state = SlotState::Downloading {
                    percent,
                    speed_mbs,
                    eta_secs,
                };
            }
        }
        SlotEvent::Done { path, size_mb } => {
            let info = app
                .slots
                .iter()
                .find(|s| s.id == slot_id)
                .map(|s| (s.title.clone(), s.artist.clone(), s.format));

            if let Some(slot) = app.slot_mut(slot_id) {
                slot.state = SlotState::Done { path: path.clone() };
            }
            if let Some((title, artist, format)) = info {
                app.push_history(HistoryEntry {
                    title: title.unwrap_or_else(|| "Unknown".to_string()),
                    artist: artist.unwrap_or_default(),
                    format,
                    size_mb,
                    path,
                    finished_at: chrono::Local::now(),
                });
            }
        }
        SlotEvent::Failed { reason } => {
            if let Some(slot) = app.slot_mut(slot_id) {
                slot.state = SlotState::Failed { reason };
            }
        }
    }
}

// ── Per-tab key handlers ──────────────────────────────────────────────────────

fn handle_queue_key(app: &mut App, key: crossterm::event::KeyEvent) {
    match key.code {
        KeyCode::Char(c) => {
            // '1'-'4' switch tabs only when the URL bar is empty (handled globally).
            // When the user is typing, digits are part of the URL — let them through.
            if app.url_input.is_empty() && matches!(c, '1' | '2' | '3' | '4') {
                return;
            }
            // [d] / [r] hotkeys only fire when the URL bar is empty.
            // If the user is typing, 'd' and 'r' are part of the URL
            // (e.g. video ID "dQw4w9WgXcQ" starts with 'd').
            if app.url_input.is_empty() {
                if c == 'd' {
                    if let Some(pos) = app
                        .slots
                        .iter()
                        .rposition(|s| matches!(s.state, SlotState::Done { .. } | SlotState::Failed { .. }))
                    {
                        app.slots.remove(pos);
                    }
                    return;
                }
                if c == 'r' {
                    if let Some(path) = app.slots.iter().rev().find_map(|s| {
                        if let SlotState::Done { path } = &s.state {
                            Some(path.clone())
                        } else {
                            None
                        }
                    }) {
                        reveal_file(app, path);
                    }
                    return;
                }
            }
            app.url_input.push(c);
        }
        KeyCode::Backspace => {
            app.url_input.pop();
        }
        KeyCode::Enter => {
            let url = normalize_url(app.url_input.trim());
            if !url.is_empty() {
                app.preview_url = url;
                app.preview_quality_cursor = 0;
                app.preview_thumbnail = None;
                app.preview_state = PreviewState::Loading;
                app.preview_fetch_needed = true; // run loop will spawn the fetch task
                app.url_input.clear();
            }
        }
        KeyCode::Delete => {
            if let Some(pos) = app
                .slots
                .iter()
                .rposition(|s| matches!(s.state, SlotState::Done { .. } | SlotState::Failed { .. }))
            {
                app.slots.remove(pos);
            }
        }
        _ => {}
    }
}

fn handle_history_key(app: &mut App, key: crossterm::event::KeyEvent) {
    match key.code {
        KeyCode::Up => {
            app.history_scroll = app.history_scroll.saturating_sub(1);
        }
        KeyCode::Down => {
            let max = app.history.len().saturating_sub(1) as u16;
            if app.history_scroll < max {
                app.history_scroll += 1;
            }
        }
        // [r] or [Enter] — reveal the selected history entry
        KeyCode::Char('r') | KeyCode::Enter => {
            let idx = app.history_scroll as usize;
            // History is displayed newest-first, so row 0 = last entry
            let path = app.history.iter().rev().nth(idx).map(|e| e.path.clone());
            if let Some(path) = path {
                reveal_file(app, path);
            }
        }
        _ => {}
    }
    app.clamp_history_scroll();
}

fn handle_lyrics_key(app: &mut App, key: crossterm::event::KeyEvent, lyrics_tx: mpsc::Sender<Option<LyricsResult>>) {
    match key.code {
        KeyCode::Char(c) => {
            // Same as Queue: digits switch tabs only when the query is empty.
            if app.lyrics_query.is_empty() && matches!(c, '1' | '2' | '3' | '4') {
                return;
            }
            app.lyrics_query.push(c);
        }
        KeyCode::Backspace => {
            app.lyrics_query.pop();
        }
        KeyCode::Up => {
            app.lyrics_scroll = app.lyrics_scroll.saturating_sub(1);
        }
        KeyCode::Down => {
            app.lyrics_scroll = app.lyrics_scroll.saturating_add(1);
        }
        KeyCode::Enter => {
            if app.lyrics_loading || app.lyrics_query.is_empty() {
                return;
            }
            let query = app.lyrics_query.clone();
            app.lyrics_loading = true;
            app.lyrics_result = None;
            app.lyrics_scroll = 0;

            // Parse "Artist - Title" or treat entire query as title
            let (artist, title) = if let Some(pos) = query.find(" - ") {
                (query[..pos].trim().to_string(), query[pos + 3..].trim().to_string())
            } else {
                (String::new(), query.clone())
            };

            // Spawn background task — never blocks the render loop
            tokio::spawn(async move {
                let result = doradura_core::lyrics::fetch_lyrics(&artist, &title).await;
                let event = result.map(|r| LyricsResult {
                    artist,
                    title,
                    lyrics: r.all_text(),
                });
                let _ = lyrics_tx.send(event).await;
            });
        }
        _ => {}
    }
}

// ── Preview popup key handler ─────────────────────────────────────────────────

fn handle_preview_key(app: &mut App, key: crossterm::event::KeyEvent, dl_tx: mpsc::Sender<(usize, SlotEvent)>) {
    // While loading: only Esc (handled globally before this fn is called)
    if matches!(app.preview_state, PreviewState::Loading) {
        return;
    }

    match key.code {
        // ← → ↑ ↓ — cycle quality for MP4
        KeyCode::Left | KeyCode::Up => {
            if app.preview_format == DownloadFormat::Mp4 {
                if let PreviewState::Ready { ref info } = app.preview_state {
                    let total = info.available_heights.len() + 1; // +1 for "best"
                    if total > 0 {
                        app.preview_quality_cursor = (app.preview_quality_cursor + total - 1) % total;
                    }
                }
            }
        }
        KeyCode::Right | KeyCode::Down => {
            if app.preview_format == DownloadFormat::Mp4 {
                if let PreviewState::Ready { ref info } = app.preview_state {
                    let total = info.available_heights.len() + 1;
                    if total > 0 {
                        app.preview_quality_cursor = (app.preview_quality_cursor + 1) % total;
                    }
                }
            }
        }

        // Tab — toggle MP3 / MP4 format inside preview
        KeyCode::Tab => {
            app.preview_format = match app.preview_format {
                DownloadFormat::Mp3 => DownloadFormat::Mp4,
                DownloadFormat::Mp4 => DownloadFormat::Mp3,
            };
            app.preview_quality_cursor = 0;
        }

        // Enter — confirm download with selected quality
        KeyCode::Enter => {
            confirm_preview_download(app, dl_tx);
        }

        _ => {}
    }
}

// ── Settings key handler ──────────────────────────────────────────────────────

fn handle_settings_key(app: &mut App, key: crossterm::event::KeyEvent, picker_tx: mpsc::Sender<String>) {
    use ui::settings::{cycle_value, get_value, set_value, ItemKind, ITEMS};

    // Global tab keys are handled above — ignore them here
    if matches!(
        key.code,
        KeyCode::Char('1') | KeyCode::Char('2') | KeyCode::Char('3') | KeyCode::Char('4')
    ) {
        return;
    }

    let cur = app.settings_cursor;
    let item = &ITEMS[cur];

    if app.settings_editing {
        // Text-edit mode
        match key.code {
            KeyCode::Enter => {
                let val = app.settings_edit_buf.clone();
                set_value(app, cur, val);
                app.settings_editing = false;
                app.settings_edit_buf.clear();
            }
            KeyCode::Backspace => {
                app.settings_edit_buf.pop();
            }
            KeyCode::Char(c) => {
                app.settings_edit_buf.push(c);
            }
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Up => {
            if app.settings_cursor > 0 {
                app.settings_cursor -= 1;
            }
        }
        KeyCode::Down => {
            if app.settings_cursor + 1 < ITEMS.len() {
                app.settings_cursor += 1;
            }
        }
        KeyCode::Left => {
            if item.kind == ItemKind::Cycle {
                cycle_value(app, cur, -1);
            }
        }
        KeyCode::Right => {
            if item.kind == ItemKind::Cycle {
                cycle_value(app, cur, 1);
            }
        }
        KeyCode::Enter => {
            if item.kind == ItemKind::Text {
                app.settings_edit_buf = get_value(app, cur);
                app.settings_editing = true;
            } else {
                // Cycle on Enter too
                cycle_value(app, cur, 1);
            }
        }
        // [o] — browse for file (text fields that are paths)
        KeyCode::Char('o') if item.kind == ItemKind::Text => {
            app.settings_file_picker_field = Some(cur);
            let tx = picker_tx.clone();
            tokio::spawn(async move {
                open_file_picker(tx).await;
            });
        }
        // [s] — save settings
        KeyCode::Char('s') => {
            if let Err(e) = app.settings.save() {
                app.error_popup = Some(format!("Failed to save settings: {}", e));
            }
        }
        // [r] — reset to defaults (confirm not needed; user can [s] to persist or quit)
        KeyCode::Char('r') => {
            app.settings = DoraSettings::default();
        }
        _ => {}
    }
}

// ── URL normalizer ────────────────────────────────────────────────────────────

/// Strip tracking/share parameters from social URLs so yt-dlp receives a clean
/// canonical URL. Currently handles YouTube (youtube.com/watch and youtu.be).
/// For all other URLs the input is returned unchanged.
fn normalize_url(url: &str) -> String {
    // youtube.com/watch?v=ID&feature=...&si=... → keep only v= (and t= if present)
    if let Some(rest) = url
        .strip_prefix("https://www.youtube.com/watch")
        .or_else(|| url.strip_prefix("http://www.youtube.com/watch"))
        .or_else(|| url.strip_prefix("https://music.youtube.com/watch"))
        .or_else(|| url.strip_prefix("http://music.youtube.com/watch"))
    {
        // Extract v= and optional t= from the query string
        let query = rest.trim_start_matches('?');
        let mut video_id = None;
        let mut time_secs = None;
        for pair in query.split('&') {
            if let Some(id) = pair.strip_prefix("v=") {
                video_id = Some(id);
            } else if let Some(t) = pair.strip_prefix("t=") {
                time_secs = Some(t);
            }
        }
        if let Some(id) = video_id {
            let host = if url.contains("music.youtube.com") {
                "https://music.youtube.com"
            } else {
                "https://www.youtube.com"
            };
            return if let Some(t) = time_secs {
                format!("{}/watch?v={}&t={}", host, id, t)
            } else {
                format!("{}/watch?v={}", host, id)
            };
        }
    }

    // youtu.be/ID?... → expand to youtube.com/watch?v=ID so yt-dlp never has to
    // follow the HTTP redirect (which YouTube appends &feature=youtu.be to, breaking
    // some yt-dlp extractor versions with "Unsupported URL").
    if let Some(path_and_query) = url
        .strip_prefix("https://youtu.be/")
        .or_else(|| url.strip_prefix("http://youtu.be/"))
    {
        let mut parts = path_and_query.splitn(2, '?');
        let id = parts.next().unwrap_or("").trim_end_matches('/');
        let time_secs = parts
            .next()
            .and_then(|q| q.split('&').find(|p| p.starts_with("t=")).map(|p| &p[2..]));
        if !id.is_empty() {
            return if let Some(t) = time_secs {
                format!("https://www.youtube.com/watch?v={}&t={}", id, t)
            } else {
                format!("https://www.youtube.com/watch?v={}", id)
            };
        }
    }

    url.to_string()
}

// ── Mouse click handler ───────────────────────────────────────────────────────

fn handle_click(app: &mut App, target: app::ClickTarget, dl_tx: mpsc::Sender<(usize, SlotEvent)>) {
    use app::ClickTarget;
    match target {
        ClickTarget::SwitchTab(tab) => {
            app.active_tab = tab;
        }
        ClickTarget::OpenInBrowser(url) => {
            open_in_browser(&url);
        }
        ClickTarget::PreviewQuality(idx) => {
            if app.preview_format == DownloadFormat::Mp4 {
                app.preview_quality_cursor = idx;
            }
        }
        ClickTarget::PreviewToggleFormat => {
            app.preview_format = match app.preview_format {
                DownloadFormat::Mp3 => DownloadFormat::Mp4,
                DownloadFormat::Mp4 => DownloadFormat::Mp3,
            };
            app.preview_quality_cursor = 0;
        }
        ClickTarget::PreviewDownload => {
            confirm_preview_download(app, dl_tx);
        }
        ClickTarget::SettingsSelectItem(idx) => {
            app.settings_cursor = idx;
        }
        ClickTarget::SettingsCycleLeft(idx) => {
            app.settings_cursor = idx;
            ui::settings::cycle_value(app, idx, -1);
            let _ = app.settings.save();
        }
        ClickTarget::SettingsCycleRight(idx) => {
            app.settings_cursor = idx;
            ui::settings::cycle_value(app, idx, 1);
            let _ = app.settings.save();
        }
        ClickTarget::LogoClick => {
            app.logo_scheme = app.logo_scheme.next();
            app.logo_burst = 100;
        }
        ClickTarget::HistorySelectRow(idx) => {
            let max = app.history.len().saturating_sub(1);
            app.history_scroll = (idx as u16).min(max as u16);
        }
    }
}

fn confirm_preview_download(app: &mut App, dl_tx: mpsc::Sender<(usize, SlotEvent)>) {
    let url = app.preview_url.clone();
    let fmt = app.preview_format;

    let quality_str = if fmt == DownloadFormat::Mp4 {
        if let PreviewState::Ready { ref info } = app.preview_state {
            let cursor = app.preview_quality_cursor;
            if cursor < info.available_heights.len() {
                format!("{}p", info.available_heights[cursor])
            } else {
                "best".to_string()
            }
        } else {
            app.settings.video_quality.clone()
        }
    } else {
        String::new()
    };

    let mut s = app.settings.clone();
    if !quality_str.is_empty() {
        s.video_quality = quality_str;
    }
    if let Some(ref c) = app.cookies_file {
        s.ytdlp_cookies = c.clone();
    }

    let id = app.add_download(url.clone(), fmt);
    if let Some(slot) = app.slot_mut(id) {
        slot.task_spawned = true;
    }
    spawn_download(id, url, fmt, s, dl_tx);

    app.preview_state = PreviewState::Hidden;
    app.preview_thumbnail = None;
}

fn open_in_browser(url: &str) {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn().ok();
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).spawn().ok();
    }
}

// ── Reveal file in Finder / show path popup ───────────────────────────────────

fn reveal_file(app: &mut App, path: String) {
    #[cfg(target_os = "macos")]
    {
        let _ = app; // not needed on macOS — Finder does everything
        std::process::Command::new("open").args(["-R", &path]).spawn().ok();
    }
    #[cfg(not(target_os = "macos"))]
    {
        app.reveal_popup = Some(path);
    }
}

// ── Native file picker (macOS) ────────────────────────────────────────────────

/// Open a native macOS "Choose File" dialog via AppleScript and send the
/// selected path back on `tx`. Silently does nothing if the user cancels
/// or if the platform is not macOS.
async fn open_file_picker(tx: mpsc::Sender<String>) {
    #[cfg(target_os = "macos")]
    {
        let script = concat!(
            "POSIX path of (choose file ",
            "with prompt \"Select your cookies.txt file\" ",
            "of type {\"public.plain-text\", \"public.data\"})",
        );
        let output = tokio::process::Command::new("osascript")
            .args(["-e", script])
            .output()
            .await;
        if let Ok(out) = output {
            if out.status.success() {
                let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !path.is_empty() {
                    let _ = tx.send(path).await;
                }
            }
        }
    }
    // On non-macOS platforms the text input already handles drag-and-drop paths.
    #[cfg(not(target_os = "macos"))]
    let _ = tx;
}
