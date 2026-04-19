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
//! | `d` / `Del`  | Remove last finished/failed slot, or cancel active |
//! | `/`          | Search history (Downloads tab)   |
//! | `?`          | Open help overlay                |
//! | `Esc`        | Close popup / clear input        |
//! | `Ctrl+C`     | Quit                             |

// The settings-menu key dispatcher uses a `match key.code { KeyCode::X => { if kind == ...`
// pattern that Rust 1.95 clippy wants collapsed to a match guard
// (`KeyCode::X if kind == ... => { ... }`). The nested style is clearer
// here because each KeyCode arm branches differently on the settings-item
// kind — collapsing would produce ~14 duplicate `KeyCode::X` arms with
// different guards. Suppressed at module scope for the TUI bin only.
#![allow(clippy::collapsible_match)]

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

use app::{
    App, DownloadFormat, HistoryEntry, LyricsResult, LyricsViewMode, PreviewState, SlotState, Tab, ToastKind,
    YtdlpStartup,
};
use download_runner::{spawn_download, SlotEvent, SubtitleOptions};
use events::{next_event, InputEvent};
use settings::DoraSettings;
use video_info::{fetch_thumbnail_art, fetch_video_info, PreviewResult};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    const VERSION: &str = env!("CARGO_PKG_VERSION");

    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("dora {}", VERSION);
        return Ok(());
    }

    let demo = args.iter().any(|a| a == "--demo");
    let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");

    if verbose {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let log_dir = std::path::PathBuf::from(&home).join(".config").join("dora");
        let _ = fs_err::create_dir_all(&log_dir);
        let log_path = log_dir.join("dora.log");
        if let Ok(file) = std::fs::File::create(&log_path) {
            let _ = WriteLogger::init(LevelFilter::Debug, LogConfig::default(), file);
            log::info!("dora started (verbose mode) — log: {}", log_path.display());
        }
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        crossterm::event::EnableMouseCapture,
        crossterm::event::EnableBracketedPaste,
    )?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut app = if demo { App::new_demo() } else { App::new() };
    // tick_rate is computed dynamically inside the loop (see needs_fast_tick).

    // Channel: background download tasks → main loop
    let (dl_tx, dl_rx) = mpsc::channel::<(usize, SlotEvent)>(256);
    // Channel: background lyrics fetch → main loop (None = not found)
    let (lyrics_tx, lyrics_rx) = mpsc::channel::<Option<LyricsResult>>(4);
    // Channel: background artist songs fetch → main loop (Option<artist_id>, Option<songs>)
    let (artist_tx, artist_rx) = mpsc::channel::<(Option<u64>, Option<Vec<doracore::lyrics::ArtistSong>>)>(4);
    // Channel: native file-picker result → main loop
    let (picker_tx, picker_rx) = mpsc::channel::<String>(4);
    // Channel: background video-info fetch → main loop
    let (preview_tx, preview_rx) = mpsc::channel::<PreviewResult>(4);
    // Channel: thumbnail art (arrives separately, after info) → main loop
    let (thumb_tx, thumb_rx) = mpsc::channel::<video_info::ThumbnailArt>(4);
    // Channel: yt-dlp update status lines → main loop
    let (ytdlp_tx, ytdlp_rx) = mpsc::channel::<String>(16);

    // Kick off yt-dlp check / update in the background
    if !demo {
        let ytdlp_bin = app.settings.ytdlp_bin.clone();
        let tx = ytdlp_tx.clone();
        tokio::spawn(async move {
            ytdlp_startup_check(ytdlp_bin, tx).await;
        });
    }

    let result = run_loop(
        &mut terminal,
        &mut app,
        dl_tx,
        dl_rx,
        lyrics_tx,
        lyrics_rx,
        artist_tx,
        artist_rx,
        picker_tx,
        picker_rx,
        preview_tx,
        preview_rx,
        thumb_tx,
        thumb_rx,
        ytdlp_rx,
    )
    .await;

    // Always restore terminal, even on error
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        crossterm::event::DisableMouseCapture,
        crossterm::event::DisableBracketedPaste,
    );
    let _ = terminal.show_cursor();

    result
}

#[allow(clippy::too_many_arguments)]
async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    dl_tx: mpsc::Sender<(usize, SlotEvent)>,
    mut dl_rx: mpsc::Receiver<(usize, SlotEvent)>,
    lyrics_tx: mpsc::Sender<Option<LyricsResult>>,
    mut lyrics_rx: mpsc::Receiver<Option<LyricsResult>>,
    artist_tx: mpsc::Sender<(Option<u64>, Option<Vec<doracore::lyrics::ArtistSong>>)>,
    mut artist_rx: mpsc::Receiver<(Option<u64>, Option<Vec<doracore::lyrics::ArtistSong>>)>,
    picker_tx: mpsc::Sender<String>,
    mut picker_rx: mpsc::Receiver<String>,
    preview_tx: mpsc::Sender<PreviewResult>,
    mut preview_rx: mpsc::Receiver<PreviewResult>,
    thumb_tx: mpsc::Sender<video_info::ThumbnailArt>,
    mut thumb_rx: mpsc::Receiver<video_info::ThumbnailArt>,
    mut ytdlp_rx: mpsc::Receiver<String>,
) -> anyhow::Result<()> {
    loop {
        // ── Draw ─────────────────────────────────────────────────────────────
        terminal.draw(|f| ui::render(f, app))?;

        // ── Drain all background-task channels (non-blocking) ────────────────
        drain_background_events(
            terminal,
            app,
            &mut dl_rx,
            &mut artist_rx,
            &mut lyrics_rx,
            &mut preview_rx,
            &mut thumb_rx,
            &mut ytdlp_rx,
            &mut picker_rx,
        );

        // ── Dispatch pending fetches + spawn tasks for queued slots ──────────
        dispatch_pending_spawns(app, &preview_tx, &thumb_tx, &dl_tx);

        // ── Input event (blocks up to tick_rate) ─────────────────────────────
        // Three tiers to avoid burning CPU when idle:
        //   • 33 ms  — active animation (downloads, particles, burst, spinners)
        //   • 500 ms — text input visible (cursor blink needs 500 ms granularity)
        //   • 10 s   — fully idle (no animation, no typing) — wake only for uptime counter
        let tick_rate = if app.needs_fast_tick() {
            Duration::from_millis(33)
        } else if app.needs_blink_tick() {
            Duration::from_millis(500)
        } else {
            Duration::from_secs(10)
        };
        match next_event(tick_rate)? {
            InputEvent::Quit => break,

            InputEvent::Tick => {
                app.tick();
            }

            InputEvent::Paste(text) => {
                handle_paste_event(app, text);
            }

            InputEvent::Mouse(mouse) => {
                handle_mouse_event(app, mouse, &dl_tx, &thumb_tx, &lyrics_tx, &artist_tx);
            }

            InputEvent::Key(key) => {
                if handle_key_event(app, key, &dl_tx, &thumb_tx, &lyrics_tx, &artist_tx, &picker_tx) {
                    break;
                }
            }
        }
    }

    Ok(())
}

// ── Background channel drainer ────────────────────────────────────────────────

/// Drain every background-task channel into `app` state (all non-blocking).
#[allow(clippy::too_many_arguments)]
fn drain_background_events(
    terminal: &Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    dl_rx: &mut mpsc::Receiver<(usize, SlotEvent)>,
    artist_rx: &mut mpsc::Receiver<(Option<u64>, Option<Vec<doracore::lyrics::ArtistSong>>)>,
    lyrics_rx: &mut mpsc::Receiver<Option<LyricsResult>>,
    preview_rx: &mut mpsc::Receiver<PreviewResult>,
    thumb_rx: &mut mpsc::Receiver<video_info::ThumbnailArt>,
    ytdlp_rx: &mut mpsc::Receiver<String>,
    picker_rx: &mut mpsc::Receiver<String>,
) {
    // ── Drain download slot events (non-blocking) ─────────────────────────
    while let Ok((slot_id, event)) = dl_rx.try_recv() {
        handle_slot_event(app, slot_id, event);
    }

    // ── Drain artist songs events (non-blocking) ──────────────────────────
    while let Ok((id_opt, result)) = artist_rx.try_recv() {
        app.lyrics_loading = false;
        if let Some(id) = id_opt {
            app.last_artist_id = Some(id);
        }
        if let Some(songs) = result {
            if app.artist_songs_page == 1 {
                app.artist_songs = songs;
                app.artist_songs_cursor = 0;
            } else {
                app.artist_songs.extend(songs);
            }
            app.lyrics_view_mode = LyricsViewMode::ArtistSongs;
        } else {
            app.add_toast("Artist songs not found", ToastKind::Error);
        }
    }

    // ── Drain lyrics events (non-blocking) ────────────────────────────────
    while let Ok(result) = lyrics_rx.try_recv() {
        app.lyrics_loading = false;
        if result.is_none() && !app.lyrics_query.is_empty() {
            app.add_toast("No lyrics found", ToastKind::Error);
        }
        app.lyrics_result = result;
    }

    // ── Drain video preview results ───────────────────────────────────────
    while let Ok(result) = preview_rx.try_recv() {
        match result {
            Ok((info, _thumb)) => {
                // Store in cache (keyed by the URL that was pending when fetch started).
                app.preview_cache.insert(app.preview_url.clone(), info.clone());
                app.preview_state = PreviewState::Ready { info };
            }
            Err(msg) => {
                app.preview_state = PreviewState::Failed(msg);
            }
        }
    }

    while let Ok(art) = thumb_rx.try_recv() {
        // Feature: Fix lag by pre-processing protocol in the background
        if let Some(picker) = &app.image_picker {
            if let Ok(img) = image::load_from_memory(&art.raw_bytes) {
                let size = terminal.size().unwrap_or_default();
                if let Ok(protocol) = picker.new_protocol(img, size.into(), ratatui_image::Resize::Fit(None)) {
                    app.preview_image_protocol = Some(protocol);
                }
            }
        }
        app.preview_thumbnail = Some(art);
    }

    // ── Drain yt-dlp startup update lines ────────────────────────────────
    while let Ok(msg) = ytdlp_rx.try_recv() {
        if msg == "__done__" {
            app.ytdlp_startup = YtdlpStartup::FadingOut { ticks: 90 };
        } else if msg == "__missing__" {
            app.ytdlp_startup = YtdlpStartup::Missing;
            app.ytdlp_available = false;
        } else {
            app.ytdlp_startup = YtdlpStartup::Updating { msg };
        }
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
}

// ── Pending-fetch dispatcher ──────────────────────────────────────────────────

/// Dispatch preview fetches (debounced + legacy immediate path) and spawn
/// download tasks for any newly-queued `Pending` slots.
fn dispatch_pending_spawns(
    app: &mut App,
    preview_tx: &mpsc::Sender<PreviewResult>,
    thumb_tx: &mpsc::Sender<video_info::ThumbnailArt>,
    dl_tx: &mpsc::Sender<(usize, SlotEvent)>,
) {
    // ── Preview debounce: dispatch fetch after 300ms of stability ─────────
    if let Some(ref pending_url) = app.preview_pending_url.clone() {
        if app.preview_debounce.elapsed() >= Duration::from_millis(300) {
            let url = pending_url.clone();
            app.preview_pending_url = None;

            // Check cache first — avoid a redundant yt-dlp -J call.
            if let Some(info) = app.preview_cache.get(&url).cloned() {
                app.preview_state = PreviewState::Ready { info };
            } else {
                app.preview_state = PreviewState::Loading;
                let ytdlp_bin = if app.settings.ytdlp_bin.trim().is_empty() {
                    "yt-dlp".to_string()
                } else {
                    app.settings.ytdlp_bin.clone()
                };
                let cookies = app.settings.cookies_opt();
                let p_tx = preview_tx.clone();
                let t_tx = thumb_tx.clone();
                tokio::spawn(async move {
                    match fetch_video_info(&url, &ytdlp_bin, cookies).await {
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
                            let _ = p_tx.send(Err(e.to_string())).await;
                        }
                    }
                });
            }
        }
    }

    // ── Legacy immediate-fetch path (set by confirm_preview_download) ─────
    if app.preview_fetch_needed {
        app.preview_fetch_needed = false;
        let url = app.preview_url.clone();
        let ytdlp_bin = if app.settings.ytdlp_bin.trim().is_empty() {
            "yt-dlp".to_string()
        } else {
            app.settings.ytdlp_bin.clone()
        };
        let cookies = app.settings.cookies_opt();
        let p_tx = preview_tx.clone();
        let t_tx = thumb_tx.clone();
        tokio::spawn(async move {
            match fetch_video_info(&url, &ytdlp_bin, cookies).await {
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
                    let _ = p_tx.send(Err(e.to_string())).await;
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
            let handle = spawn_download(slot.id, slot.url.clone(), slot.format, s, dl_tx.clone(), None);
            slot.cancel = Some(handle);
        }
    }
}

// ── Paste event handler ───────────────────────────────────────────────────────

fn handle_paste_event(app: &mut App, text: String) {
    let clean = text.trim().to_string();
    if app.show_cookies_input {
        app.cookies_input.push_str(&clean);
    } else if app.settings_editing {
        app.settings_edit_buf.push_str(&clean);
    } else if app.active_tab == Tab::Lyrics {
        app.lyrics_query.push_str(&clean);
    } else if app.active_tab == Tab::Downloads && !app.preview_state.is_visible() {
        let urls: Vec<String> = text
            .split('\n')
            .map(str::trim)
            .filter(|s| s.starts_with("http://") || s.starts_with("https://"))
            .map(normalize_url)
            .collect();
        match urls.len() {
            0 => {
                // No valid URLs — append raw text to url_input
                app.url_input.push_str(&clean);
            }
            1 => {
                // Single URL: open preview like pressing Enter.
                // Arm `1 =>` proves `urls.len() == 1`, so .next() yields Some.
                let url = urls.into_iter().next().expect("match arm guarantees urls.len() == 1");
                app.preview_url = url.clone();
                app.preview_format = DownloadFormat::Mp4;
                app.preview_quality_cursor = 0;
                app.preview_thumbnail = None;
                app.preview_image_protocol = None;
                app.preview_state = PreviewState::Loading;
                app.preview_pending_url = Some(url);
                app.preview_debounce = std::time::Instant::now();
                app.url_input.clear();
            }
            count => {
                // Multiple URLs: queue all directly without preview
                let fmt = if app.settings.default_format == "MP4" {
                    DownloadFormat::Mp4
                } else {
                    DownloadFormat::Mp3
                };
                for url in urls {
                    app.add_download(url, fmt);
                }
                app.preview_image_protocol = None;
                app.add_toast(&format!("Queued {} downloads", count), ToastKind::Success);
            }
        }
    }
}

// ── Mouse event handler ───────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn handle_mouse_event(
    app: &mut App,
    mouse: crossterm::event::MouseEvent,
    dl_tx: &mpsc::Sender<(usize, SlotEvent)>,
    thumb_tx: &mpsc::Sender<video_info::ThumbnailArt>,
    lyrics_tx: &mpsc::Sender<Option<LyricsResult>>,
    artist_tx: &mpsc::Sender<(Option<u64>, Option<Vec<doracore::lyrics::ArtistSong>>)>,
) {
    use crossterm::event::{MouseButton, MouseEventKind};
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let col = mouse.column;
            let row = mouse.row;
            // Spawn a small ripple at the click point.
            let theme = app.theme;
            let ripple = app::spawn_ripple_particles(col as f32, row as f32, &theme);
            app.particles.extend(ripple);
            let targets: Vec<_> = app.click_map.clone();
            // Iterate in reverse: later-registered (more specific) targets
            // take priority over earlier broad ones (e.g. ← → over whole row).
            for (rect, target) in targets.into_iter().rev() {
                if col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height {
                    handle_click(
                        app,
                        target,
                        dl_tx.clone(),
                        thumb_tx.clone(),
                        lyrics_tx.clone(),
                        artist_tx.clone(),
                    );
                    break;
                }
            }
        }
        MouseEventKind::ScrollDown => match app.active_tab {
            Tab::Downloads => {
                let max = app.history_filtered_indices.len().saturating_sub(1);
                if (app.history_scroll as usize) < max {
                    app.history_scroll += 1;
                    // Keep index in sync or move it if it's out of view
                    if app.history_index < app.history_scroll as usize {
                        app.history_index = app.history_scroll as usize;
                    }
                }
            }
            Tab::Lyrics => {
                app.lyrics_scroll = app.lyrics_scroll.saturating_add(3);
            }
            _ => {}
        },
        MouseEventKind::ScrollUp => match app.active_tab {
            Tab::Downloads => {
                app.history_scroll = app.history_scroll.saturating_sub(1);
                let visible_rows = 15;
                if app.history_index >= (app.history_scroll + visible_rows) as usize {
                    app.history_index = (app.history_scroll + visible_rows - 1) as usize;
                }
            }
            Tab::Lyrics => {
                app.lyrics_scroll = app.lyrics_scroll.saturating_sub(3);
            }
            _ => {}
        },
        _ => {}
    }
}

// ── Key event dispatcher ──────────────────────────────────────────────────────

/// Handle one key event. Returns `true` when the caller should break out of the main loop (quit).
#[allow(clippy::too_many_arguments)]
fn handle_key_event(
    app: &mut App,
    key: crossterm::event::KeyEvent,
    dl_tx: &mpsc::Sender<(usize, SlotEvent)>,
    thumb_tx: &mpsc::Sender<video_info::ThumbnailArt>,
    lyrics_tx: &mpsc::Sender<Option<LyricsResult>>,
    artist_tx: &mpsc::Sender<(Option<u64>, Option<Vec<doracore::lyrics::ArtistSong>>)>,
    picker_tx: &mpsc::Sender<String>,
) -> bool {
    // ── yt-dlp missing popup intercepts ALL keys ──────────────────
    if app.ytdlp_startup == YtdlpStartup::Missing {
        match key.code {
            KeyCode::Char('i') => {
                // Open yt-dlp releases page in browser
                open_in_browser("https://github.com/yt-dlp/yt-dlp/releases/latest");
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                return true; // quit the app
            }
            _ => {}
        }
        return false;
    }

    // ── Esc: universal dismiss ────────────────────────────────────
    if key.code == KeyCode::Esc {
        if app.show_cookies_input {
            app.show_cookies_input = false;
            app.cookies_input.clear();
        } else if app.help_visible {
            app.help_visible = false;
        } else if app.history_popup.is_some() {
            app.history_popup = None;
        } else if app.reveal_popup.is_some() {
            app.reveal_popup = None;
        } else if app.preview_subs_editing {
            app.preview_subs_editing = false;
            app.preview_subs_edit_buf.clear();
        } else if app.preview_subs_menu {
            app.preview_subs_menu = false;
        } else if app.preview_state.is_visible() {
            // Cancel preview — restore URL input
            let url = std::mem::take(&mut app.preview_url);
            app.url_input = url;
            app.preview_state = PreviewState::Hidden;
            app.preview_thumbnail = None;
            app.preview_image_protocol = None;
            app.preview_pending_url = None;
            app.preview_subs_menu = false;
            app.preview_subs_enabled = false;
            app.preview_subs_lang_cursor = 0;
            app.preview_subs_custom_lang = None;
            app.preview_subs_editing = false;
            app.preview_subs_edit_buf.clear();
        } else if app.settings_editing {
            app.settings_editing = false;
            app.settings_edit_buf.clear();
        } else if app.history_search_mode {
            // Esc clears history filter
            app.history_search_mode = false;
            app.history_filter.clear();
        } else {
            match app.active_tab {
                Tab::Downloads => app.url_input.clear(),
                Tab::Lyrics => app.lyrics_query.clear(),
                Tab::Settings => {}
            }
        }
        return false;
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
        return false;
    }

    // ── History search mode intercepts text input ─────────────────
    if app.history_search_mode {
        match key.code {
            KeyCode::Backspace => {
                app.history_filter.pop();
                if app.history_filter.is_empty() {
                    app.history_search_mode = false;
                }
                app.update_history_filter();
            }
            KeyCode::Enter => {
                // Lock in the filter; search mode stays active but Enter stops editing
                app.history_search_mode = false;
            }
            KeyCode::Char(c) => {
                app.history_filter.push(c);
                app.update_history_filter();
            }
            _ => {}
        }
        return false;
    }

    // ── History detail popup intercepts keys ─────────────────────
    if let Some(idx) = app.history_popup {
        match key.code {
            KeyCode::Char('r') | KeyCode::Enter => {
                if let Some(entry) = app.history.iter().rev().nth(idx) {
                    let path = entry.path.clone();
                    app.history_popup = None;
                    app.preview_thumbnail = None;
                    app.preview_image_protocol = None;
                    reveal_file(app, path);
                }
            }
            KeyCode::Char('b') => {
                if let Some(entry) = app.history.iter().rev().nth(idx) {
                    if !entry.url.is_empty() {
                        let url = entry.url.clone();
                        open_in_browser(&url);
                    }
                }
            }
            KeyCode::Char('d') => {
                // Remove this entry from history (display-index → vec index)
                let vec_idx = app.history.len().saturating_sub(1).saturating_sub(idx);
                if vec_idx < app.history.len() {
                    app.history.remove(vec_idx);
                    app.history_save();
                }
                app.history_popup = None;
                app.preview_thumbnail = None;
                app.preview_image_protocol = None;
                app.clamp_history_scroll();
            }
            KeyCode::Esc => {
                app.history_popup = None;
                app.preview_thumbnail = None;
                app.preview_image_protocol = None;
            }
            _ => {}
        }
        return false;
    }

    // ── Any key dismisses reveal popup ────────────────────────────
    if app.reveal_popup.is_some() {
        app.reveal_popup = None;
        return false;
    }

    // ── Preview popup intercepts all keys ─────────────────────────
    if app.preview_state.is_visible() {
        handle_preview_key(app, key, dl_tx.clone(), thumb_tx.clone());
        return false;
    }

    // ── Any key closes help overlay ───────────────────────────────
    if app.help_visible {
        app.help_visible = false;
        return false;
    }

    // ── 'T' cycles Catppuccin theme ──────────────────────────────
    if key.code == KeyCode::Char('T') {
        app.settings.theme_flavour = app.settings.theme_flavour.next();
        app.theme = crate::theme::palette(app.settings.theme_flavour);
        let _ = app.settings.save();
        app.add_toast(
            &format!("Theme: {}", app.settings.theme_flavour.label()),
            ToastKind::Info,
        );
        return false;
    }

    // ── '?' opens help (only when not typing in a text input) ────
    if key.code == KeyCode::Char('?') {
        let typing = (app.active_tab == Tab::Downloads && !app.url_input.trim().is_empty())
            || (app.active_tab == Tab::Lyrics && !app.lyrics_query.trim().is_empty());
        if !typing {
            app.help_visible = true;
            return false;
        }
    }

    // ── '/' in Downloads tab — activate history search ────────────
    if app.active_tab == Tab::Downloads && app.url_input.trim().is_empty() && key.code == KeyCode::Char('/') {
        app.history_search_mode = true;
        app.history_filter.clear();
        return false;
    }

    // ── 'c' opens cookies popup (Downloads tab, only when not typing) ───
    if app.active_tab == Tab::Downloads && app.url_input.trim().is_empty() && key.code == KeyCode::Char('c') {
        app.show_cookies_input = true;
        app.cookies_input = app.cookies_file.clone().unwrap_or_default();
        return false;
    }

    // ── Tab-specific handling ─────────────────────────────────────
    match app.active_tab {
        Tab::Downloads => handle_downloads_key(app, key, dl_tx.clone(), thumb_tx.clone()),
        Tab::Lyrics => handle_lyrics_key(app, key, lyrics_tx.clone(), artist_tx.clone()),
        Tab::Settings => handle_settings_key(app, key, picker_tx.clone()),
    }

    // ── Global tab-switching (suppressed while typing in a text input) ──
    let typing_in_input = (app.active_tab == Tab::Downloads && !app.url_input.trim().is_empty())
        || (app.active_tab == Tab::Lyrics && !app.lyrics_query.trim().is_empty());
    if !typing_in_input {
        match key.code {
            KeyCode::Char('1') => app.active_tab = Tab::Downloads,
            KeyCode::Char('2') => app.active_tab = Tab::Lyrics,
            KeyCode::Char('3') => app.active_tab = Tab::Settings,
            _ => {}
        }
    }

    false
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
                // Keep a ring buffer of the last 20 speed samples.
                slot.speed_history.push_back(speed_mbs);
                if slot.speed_history.len() > 20 {
                    slot.speed_history.pop_front();
                }
            }
        }
        SlotEvent::BurningSubtitles => {
            if let Some(slot) = app.slot_mut(slot_id) {
                slot.state = SlotState::Downloading {
                    percent: 99,
                    speed_mbs: 0.0,
                    eta_secs: 0,
                };
            }
            app.add_toast("Burning subtitles...", ToastKind::Info);
        }
        SlotEvent::Done { path, size_mb } => {
            let info = app
                .slots
                .iter()
                .find(|s| s.id == slot_id)
                .map(|s| (s.title.clone(), s.artist.clone(), s.format, s.url.clone()));

            if let Some(slot) = app.slot_mut(slot_id) {
                // Transition through Celebrating first (1-second animation).
                slot.state = SlotState::Celebrating {
                    path: path.clone(),
                    started: std::time::Instant::now(),
                };
                slot.cancel = None; // task is done, handle is no longer valid
            }

            // Feature: TUI Toasts (Done notification)
            if let Some((title, _, _, _)) = &info {
                let name = title.as_deref().unwrap_or("Media");
                app.add_toast(&format!("Done: {}", name), ToastKind::Success);
            }

            if let Some((title, artist, format, url)) = info {
                let thumb_url = app
                    .slots
                    .iter()
                    .find(|s| s.id == slot_id)
                    .and_then(|s| s.thumbnail_url.clone());
                app.push_history(HistoryEntry {
                    title: title.unwrap_or_else(|| "Unknown".to_string()),
                    artist: artist.unwrap_or_default(),
                    format,
                    size_mb,
                    path,
                    finished_at: chrono::Local::now(),
                    url,
                    thumbnail_url: thumb_url,
                });
            }
        }
        SlotEvent::Failed { reason } => {
            let short: String = reason.chars().take(80).collect();
            app.add_toast(&format!("Download failed: {}", short), ToastKind::Error);
            if let Some(slot) = app.slot_mut(slot_id) {
                slot.state = SlotState::Failed { reason };
                slot.cancel = None;
            }
        }
    }
}

// ── Per-tab key handlers ──────────────────────────────────────────────────────

/// Combined Downloads tab handler — URL queue + history navigation.
fn handle_downloads_key(
    app: &mut App,
    key: crossterm::event::KeyEvent,
    dl_tx: mpsc::Sender<(usize, SlotEvent)>,
    thumb_tx: mpsc::Sender<video_info::ThumbnailArt>,
) {
    // History interaction takes priority when URL bar is empty
    if app.url_input.trim().is_empty() && !app.history.is_empty() {
        match key.code {
            KeyCode::Up | KeyCode::Down | KeyCode::Char(' ') | KeyCode::Char('r') | KeyCode::Enter => {
                handle_history_key(app, key, dl_tx, thumb_tx);
                return;
            }
            // [s] — cycle history sort order
            KeyCode::Char('s') => {
                app.history_sort = app.history_sort.next();
                app.update_history_filter();
                return;
            }
            _ => {}
        }
    }
    handle_queue_key(app, key, dl_tx);
}

fn handle_queue_key(app: &mut App, key: crossterm::event::KeyEvent, _dl_tx: mpsc::Sender<(usize, SlotEvent)>) {
    match key.code {
        KeyCode::Char(c) => {
            // '1'-'3' switch tabs only when the URL bar is empty (handled globally).
            // When the user is typing, digits are part of the URL — let them through.
            if app.url_input.is_empty() && matches!(c, '1' | '2' | '3') {
                return;
            }
            // [d] / [r] hotkeys only fire when the URL bar is empty.
            if app.url_input.is_empty() {
                if c == 'd' {
                    remove_or_cancel_slot(app);
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
                // Set up debounced preview fetch (300ms).
                app.preview_url = url.clone();
                app.preview_format = DownloadFormat::Mp4;
                app.preview_quality_cursor = 0;
                app.preview_thumbnail = None;
                app.preview_image_protocol = None;
                app.preview_state = PreviewState::Loading;
                app.preview_pending_url = Some(url);
                app.preview_debounce = std::time::Instant::now();
                app.url_input.clear();
            }
        }
        KeyCode::Delete => {
            remove_or_cancel_slot(app);
        }
        _ => {}
    }
}

/// Remove the last finished/failed/celebrating slot, or abort + remove the last active slot.
fn remove_or_cancel_slot(app: &mut App) {
    // Prefer removing a terminal-state slot first.
    if let Some(pos) = app.slots.iter().rposition(|s| {
        matches!(
            s.state,
            SlotState::Done { .. } | SlotState::Failed { .. } | SlotState::Celebrating { .. }
        )
    }) {
        app.slots.remove(pos);
        return;
    }
    // Otherwise abort the last active slot.
    if let Some(pos) = app.slots.iter().rposition(|s| {
        matches!(
            s.state,
            SlotState::Downloading { .. } | SlotState::Fetching | SlotState::Pending
        )
    }) {
        if let Some(handle) = app.slots[pos].cancel.take() {
            handle.abort();
        }
        app.slots.remove(pos);
    }
}

fn handle_history_key(
    app: &mut App,
    key: crossterm::event::KeyEvent,
    dl_tx: mpsc::Sender<(usize, SlotEvent)>,
    thumb_tx: mpsc::Sender<video_info::ThumbnailArt>,
) {
    let num_entries = app.history_filtered_indices.len();
    if num_entries == 0 {
        return;
    }

    match key.code {
        KeyCode::Up => {
            if app.history_index > 0 {
                app.history_index -= 1;
            }
        }
        KeyCode::Down => {
            if app.history_index + 1 < num_entries {
                app.history_index += 1;
            }
        }
        KeyCode::Char(' ') => {
            let filtered_pos = app.history_index;
            if let Some(&display_idx) = app.history_filtered_indices.get(filtered_pos) {
                if app.history_selected.contains(&display_idx) {
                    app.history_selected.remove(&display_idx);
                } else {
                    app.history_selected.insert(display_idx);
                }
            }
            return;
        }
        KeyCode::Char('r') | KeyCode::Enter => {
            let filtered_pos = app.history_index;
            handle_click_internal(
                app,
                app::ClickTarget::HistoryOpenPopup(filtered_pos),
                dl_tx,
                thumb_tx,
                mpsc::channel(1).0,
                mpsc::channel(1).0,
            );
            return;
        }
        _ => {}
    }

    // Auto-scroll: keep history_index in view
    let scroll = app.history_scroll as usize;
    let view_h = 15;
    if app.history_index < scroll {
        app.history_scroll = app.history_index as u16;
    } else if app.history_index >= scroll + view_h {
        app.history_scroll = (app.history_index + 1 - view_h) as u16;
    }
}

fn handle_lyrics_key(
    app: &mut App,
    key: crossterm::event::KeyEvent,
    lyrics_tx: mpsc::Sender<Option<LyricsResult>>,
    artist_tx: mpsc::Sender<(Option<u64>, Option<Vec<doracore::lyrics::ArtistSong>>)>,
) {
    if app.lyrics_view_mode == app::LyricsViewMode::ArtistSongs {
        let card_w = 30usize;
        let cols = (80 / card_w).max(1);

        match key.code {
            KeyCode::Esc => {
                app.lyrics_view_mode = app::LyricsViewMode::Lyrics;
            }
            KeyCode::Up => {
                if app.artist_songs_cursor >= cols {
                    app.artist_songs_cursor -= cols;
                }
            }
            KeyCode::Down => {
                if app.artist_songs_cursor + cols < app.artist_songs.len() {
                    app.artist_songs_cursor += cols;
                }
            }
            KeyCode::Left => {
                if app.artist_songs_cursor > 0 {
                    app.artist_songs_cursor -= 1;
                }
            }
            KeyCode::Right => {
                if app.artist_songs_cursor < app.artist_songs.len() {
                    app.artist_songs_cursor += 1;
                }
            }
            KeyCode::Char('m') => {
                handle_click_internal(
                    app,
                    app::ClickTarget::LyricsLoadMore,
                    mpsc::channel(1).0,
                    mpsc::channel(1).0,
                    lyrics_tx.clone(),
                    artist_tx.clone(),
                );
            }
            KeyCode::Enter => {
                if app.artist_songs_cursor == app.artist_songs.len() {
                    handle_click_internal(
                        app,
                        app::ClickTarget::LyricsLoadMore,
                        mpsc::channel(1).0,
                        mpsc::channel(1).0,
                        lyrics_tx.clone(),
                        artist_tx.clone(),
                    );
                } else if let Some(song) = app.artist_songs.get(app.artist_songs_cursor).cloned() {
                    handle_click_internal(
                        app,
                        app::ClickTarget::ArtistSongClick(song.artist, song.title),
                        mpsc::channel(1).0,
                        mpsc::channel(1).0,
                        lyrics_tx.clone(),
                        mpsc::channel(1).0,
                    );
                }
            }
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Char(c) => {
            // Digits switch tabs only when the query is empty.
            if app.lyrics_query.is_empty() && matches!(c, '1' | '2' | '3') {
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
            if app.lyrics_loading || app.lyrics_query.trim().is_empty() {
                return;
            }
            let query = app.lyrics_query.clone();

            if app.demo_mode {
                app.lyrics_loading = true;
                app.artist_songs_page = 1;
                app.last_lyrics_query = query.clone();
                app.last_artist_id = None;
                let a_tx = artist_tx.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    let mock_songs = vec![
                        doracore::lyrics::ArtistSong {
                            id: 1,
                            title: format!("{} (Remix)", query),
                            artist: "Dora Demo".to_string(),
                            thumbnail_url: None,
                        },
                        doracore::lyrics::ArtistSong {
                            id: 2,
                            title: format!("{} - Live", query),
                            artist: "Dora Demo".to_string(),
                            thumbnail_url: None,
                        },
                        doracore::lyrics::ArtistSong {
                            id: 3,
                            title: "Bohemian Rhapsody".to_string(),
                            artist: "Queen".to_string(),
                            thumbnail_url: None,
                        },
                        doracore::lyrics::ArtistSong {
                            id: 4,
                            title: "Never Gonna Give You Up".to_string(),
                            artist: "Rick Astley".to_string(),
                            thumbnail_url: None,
                        },
                    ];
                    let _ = a_tx.send((None, Some(mock_songs))).await;
                });
                return;
            }

            let g_token = app.settings.genius_token.clone();

            if g_token.is_empty() && doracore::core::config::GENIUS_CLIENT_TOKEN.is_none() {
                app.add_toast("Genius token missing in Settings", ToastKind::Error);
                return;
            }

            app.lyrics_loading = true;
            app.artist_songs_page = 1;
            app.last_lyrics_query = query.clone();
            app.last_artist_id = None;

            let a_tx = artist_tx.clone();
            tokio::spawn(async move {
                let token = if g_token.is_empty() {
                    doracore::core::config::GENIUS_CLIENT_TOKEN
                        .as_ref()
                        .cloned()
                        .unwrap_or_default()
                } else {
                    g_token
                };
                let result = doracore::lyrics::fetch_search_results(&query, &token, 1).await;
                let _ = a_tx.send((None, result)).await;
            });
        }
        _ => {}
    }
}

// ── Preview popup key handler ─────────────────────────────────────────────────

fn handle_preview_key(
    app: &mut App,
    key: crossterm::event::KeyEvent,
    dl_tx: mpsc::Sender<(usize, SlotEvent)>,
    _thumb_tx: mpsc::Sender<video_info::ThumbnailArt>,
) {
    // While loading: only Esc (handled globally before this fn is called)
    if matches!(app.preview_state, PreviewState::Loading) {
        return;
    }

    // Custom language text input mode
    if app.preview_subs_editing {
        match key.code {
            KeyCode::Enter => {
                let lang = app.preview_subs_edit_buf.trim().to_string();
                if !lang.is_empty() {
                    app.preview_subs_custom_lang = Some(lang);
                    app.preview_subs_enabled = true;
                }
                app.preview_subs_editing = false;
                app.preview_subs_edit_buf.clear();
            }
            KeyCode::Esc => {
                app.preview_subs_editing = false;
                app.preview_subs_edit_buf.clear();
            }
            KeyCode::Backspace => {
                app.preview_subs_edit_buf.pop();
            }
            KeyCode::Char(c) => {
                if app.preview_subs_edit_buf.len() < 10 {
                    app.preview_subs_edit_buf.push(c);
                }
            }
            _ => {}
        }
        return;
    }

    // Subtitle sub-menu keys
    if app.preview_subs_menu {
        match key.code {
            KeyCode::Left => {
                if let PreviewState::Ready { ref info } = app.preview_state {
                    if !info.subtitle_langs.is_empty() {
                        app.preview_subs_custom_lang = None;
                        app.preview_subs_enabled = true;
                        let total = info.subtitle_langs.len();
                        app.preview_subs_lang_cursor = (app.preview_subs_lang_cursor + total - 1) % total;
                    }
                }
            }
            KeyCode::Right => {
                if let PreviewState::Ready { ref info } = app.preview_state {
                    if !info.subtitle_langs.is_empty() {
                        app.preview_subs_custom_lang = None;
                        app.preview_subs_enabled = true;
                        let total = info.subtitle_langs.len();
                        app.preview_subs_lang_cursor = (app.preview_subs_lang_cursor + 1) % total;
                    }
                }
            }
            KeyCode::Char(' ') => {
                app.preview_subs_enabled = !app.preview_subs_enabled;
            }
            KeyCode::Char('c') => {
                app.preview_subs_editing = true;
                app.preview_subs_edit_buf.clear();
            }
            KeyCode::Esc | KeyCode::Enter => {
                app.preview_subs_menu = false;
            }
            _ => {}
        }
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

        // S — open subtitle menu (MP4 only); auto-enables subs
        KeyCode::Char('s') | KeyCode::Char('S') => {
            if app.preview_format == DownloadFormat::Mp4 {
                app.preview_subs_menu = true;
                app.preview_subs_enabled = true;
            }
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
    if matches!(key.code, KeyCode::Char('1') | KeyCode::Char('2') | KeyCode::Char('3')) {
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
                // Auto-save after confirming a text edit.
                let _ = app.settings.save();
                app.add_toast("Settings saved", ToastKind::Success);
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
                let _ = app.settings.save();
            }
        }
        KeyCode::Right => {
            if item.kind == ItemKind::Cycle {
                cycle_value(app, cur, 1);
                let _ = app.settings.save();
            }
        }
        KeyCode::Enter => {
            if item.kind == ItemKind::Text {
                app.settings_edit_buf = get_value(app, cur);
                app.settings_editing = true;
            } else {
                // Cycle on Enter too
                cycle_value(app, cur, 1);
                let _ = app.settings.save();
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
        // [g] — get Genius token (only for Genius field)
        KeyCode::Char('g') if cur == 12 => {
            open_in_browser("https://genius.com/api-clients");
        }
        // [s] — explicit save (still works; also shows confirmation)
        KeyCode::Char('s') => {
            if let Err(e) = app.settings.save() {
                app.add_toast(&format!("Save failed: {}", e), ToastKind::Error);
            } else {
                app.add_toast("Settings saved", ToastKind::Success);
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
    // follow the HTTP redirect.
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

// ── yt-dlp startup check / update ────────────────────────────────────────────

/// Checks for yt-dlp binary presence and runs `yt-dlp -U` if found.
/// Sends status strings through `tx`; "__done__" and "__missing__" are
/// sentinel values consumed by the main loop.
async fn ytdlp_startup_check(ytdlp_bin: String, tx: mpsc::Sender<String>) {
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::process::Command;

    // Check if the binary exists at all
    let version_ok = Command::new(&ytdlp_bin)
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !version_ok {
        let _ = tx.send("__missing__".to_string()).await;
        return;
    }

    // Binary found — run `yt-dlp -U` to self-update
    let mut child = match Command::new(&ytdlp_bin)
        .args(["-U", "--no-color"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => {
            let _ = tx.send("__done__".to_string()).await;
            return;
        }
    };

    if let Some(stdout) = child.stdout.take() {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let trimmed = line.trim().to_string();
            if !trimmed.is_empty() {
                let _ = tx.send(trimmed).await;
            }
        }
    }

    let _ = child.wait().await;
    let _ = tx.send("__done__".to_string()).await;
}

// ── Mouse click handler ───────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn handle_click(
    app: &mut App,
    target: app::ClickTarget,
    dl_tx: mpsc::Sender<(usize, SlotEvent)>,
    thumb_tx: mpsc::Sender<video_info::ThumbnailArt>,
    lyrics_tx: mpsc::Sender<Option<LyricsResult>>,
    artist_tx: mpsc::Sender<(Option<u64>, Option<Vec<doracore::lyrics::ArtistSong>>)>,
) {
    handle_click_internal(app, target, dl_tx, thumb_tx, lyrics_tx, artist_tx);
}

#[allow(clippy::too_many_arguments)]
fn handle_click_internal(
    app: &mut App,
    target: app::ClickTarget,
    dl_tx: mpsc::Sender<(usize, SlotEvent)>,
    thumb_tx: mpsc::Sender<video_info::ThumbnailArt>,
    lyrics_tx: mpsc::Sender<Option<LyricsResult>>,
    artist_tx: mpsc::Sender<(Option<u64>, Option<Vec<doracore::lyrics::ArtistSong>>)>,
) {
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
        ClickTarget::PreviewClose => {
            app.preview_state = app::PreviewState::Hidden;
            app.history_popup = None;
            app.preview_thumbnail = None;
            app.preview_image_protocol = None;
            app.preview_pending_url = None;
            app.preview_subs_menu = false;
            app.preview_subs_editing = false;
        }
        ClickTarget::PreviewToggleSubsMenu => {
            app.preview_subs_menu = !app.preview_subs_menu;
        }
        ClickTarget::PreviewToggleSubsEnabled => {
            app.preview_subs_enabled = !app.preview_subs_enabled;
        }
        ClickTarget::PreviewSubsLang(idx) => {
            app.preview_subs_lang_cursor = idx;
            app.preview_subs_custom_lang = None;
            app.preview_subs_enabled = true;
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
            app.settings.logo_scheme = app.logo_scheme;
            let _ = app.settings.save();
            app.logo_burst = 100;
        }
        ClickTarget::HistoryOpenPopup(filtered_pos) => {
            if app.history_index == filtered_pos {
                // Second click on already-selected row — open popup
                if let Some(&raw_idx) = app.history_filtered_indices.get(filtered_pos) {
                    app.history_popup = Some(raw_idx);

                    // Feature: Real History Preview
                    if let Some(entry) = app.history.iter().rev().nth(raw_idx) {
                        if let Some(turl) = &entry.thumbnail_url {
                            app.preview_thumbnail = None;
                            app.preview_image_protocol = None;
                            let turl = turl.clone();
                            let t_tx = thumb_tx.clone();
                            tokio::spawn(async move {
                                if let Some(art) = fetch_thumbnail_art(&turl).await {
                                    let _ = t_tx.send(art).await;
                                }
                            });
                        }
                    }
                }
            } else {
                // First click — just select the row
                app.history_index = filtered_pos;
            }
        }
        ClickTarget::HistoryReveal(path) => {
            app.history_popup = None;
            app.preview_thumbnail = None;
            app.preview_image_protocol = None;
            reveal_file(app, path);
        }
        ClickTarget::ArtistClick(id_opt, name) => {
            if app.demo_mode {
                app.lyrics_loading = true;
                app.artist_songs_page = 1;
                app.last_lyrics_query.clear();
                app.last_artist_id = Some(123);
                let a_tx = artist_tx.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    let mock_songs = (1..=10)
                        .map(|i| doracore::lyrics::ArtistSong {
                            id: i as u64,
                            title: format!("Greatest Hit Vol. {}", i),
                            artist: name.clone(),
                            thumbnail_url: None,
                        })
                        .collect();
                    let _ = a_tx.send((Some(123), Some(mock_songs))).await;
                });
                return;
            }

            let g_token = app.settings.genius_token.clone();
            if g_token.is_empty() && doracore::core::config::GENIUS_CLIENT_TOKEN.is_none() {
                app.add_toast("Genius token missing in Settings", ToastKind::Error);
                return;
            }
            app.lyrics_loading = true;
            app.artist_songs_page = 1;
            app.last_lyrics_query.clear();

            let a_tx = artist_tx.clone();
            tokio::spawn(async move {
                let token = if g_token.is_empty() {
                    doracore::core::config::GENIUS_CLIENT_TOKEN
                        .as_ref()
                        .cloned()
                        .unwrap_or_default()
                } else {
                    g_token
                };
                let artist_id = match id_opt {
                    Some(id) => Some(id),
                    None => doracore::lyrics::fetch_artist_id(&name, &token).await,
                };
                if let Some(id) = artist_id {
                    let result = doracore::lyrics::fetch_artist_songs(id, &token, 1).await;
                    let _ = a_tx.send((Some(id), result)).await;
                } else {
                    let _ = a_tx.send((None, None)).await;
                }
            });
        }
        ClickTarget::ArtistSongClick(artist, title) => {
            app.lyrics_query = format!("{} - {}", artist, title);
            app.lyrics_loading = true;
            app.lyrics_result = None;
            app.lyrics_scroll = 0;
            app.lyrics_view_mode = app::LyricsViewMode::Lyrics;

            if app.demo_mode {
                let l_tx = lyrics_tx.clone();
                let artist_clone = artist.clone();
                let title_clone = title.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    let _ = l_tx.send(Some(app::LyricsResult {
                        artist: artist_clone,
                        artist_id: Some(123),
                        title: title_clone,
                        album: Some("Demo Album".to_string()),
                        release_date: Some("2026-03-04".to_string()),
                        thumbnail_url: None,
                        lyrics: "This is a demo lyrics text.\n\n[Verse 1]\nIt works without a token!\nIn demo mode you see this.\n\n[Chorus]\nCards are beautiful!\nGrid is responsive!\nLoad more is fun!\n\n[Outro]\nEnjoy dora-tui!".to_string(),
                    })).await;
                });
                return;
            }

            let l_tx = lyrics_tx.clone();
            let g_token = if app.settings.genius_token.is_empty() {
                None
            } else {
                Some(app.settings.genius_token.clone())
            };
            tokio::spawn(async move {
                let result = doracore::lyrics::fetch_lyrics(&artist, &title, g_token.as_deref()).await;
                let event = result.map(|r| {
                    let lyrics = r.all_text();
                    app::LyricsResult {
                        artist: r.artist,
                        artist_id: r.artist_id,
                        title: r.title,
                        album: r.album,
                        release_date: r.release_date,
                        thumbnail_url: r.thumbnail_url,
                        lyrics,
                    }
                });
                let _ = l_tx.send(event).await;
            });
        }
        ClickTarget::GetGeniusToken => {
            open_in_browser("https://genius.com/api-clients");
        }
        ClickTarget::LyricsLoadMore => {
            if app.lyrics_loading {
                return;
            }

            if app.demo_mode {
                app.lyrics_loading = true;
                app.artist_songs_page += 1;
                let page = app.artist_songs_page;
                let a_id = app.last_artist_id;
                let a_tx = artist_tx.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    let mock_songs = (1..=10)
                        .map(|i| doracore::lyrics::ArtistSong {
                            id: (page as u64 * 100) + i as u64,
                            title: format!("Bonus Track #{}", (page - 1) * 10 + i),
                            artist: "Dora Demo".to_string(),
                            thumbnail_url: None,
                        })
                        .collect();
                    let _ = a_tx.send((a_id, Some(mock_songs))).await;
                });
                return;
            }

            let g_token = app.settings.genius_token.clone();
            if g_token.is_empty() && doracore::core::config::GENIUS_CLIENT_TOKEN.is_none() {
                app.add_toast("Genius token missing", ToastKind::Error);
                return;
            }

            app.lyrics_loading = true;
            app.artist_songs_page += 1;
            let page = app.artist_songs_page;
            let a_id = app.last_artist_id;
            let query = app.last_lyrics_query.clone();

            let a_tx = artist_tx.clone();
            tokio::spawn(async move {
                let token = if g_token.is_empty() {
                    doracore::core::config::GENIUS_CLIENT_TOKEN
                        .as_ref()
                        .cloned()
                        .unwrap_or_default()
                } else {
                    g_token
                };

                let result = if let Some(id) = a_id {
                    doracore::lyrics::fetch_artist_songs(id, &token, page).await
                } else {
                    doracore::lyrics::fetch_search_results(&query, &token, page).await
                };
                let _ = a_tx.send((a_id, result)).await;
            });
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

    // Build subtitle options if enabled and format is MP4
    let subtitle_opts = if app.preview_subs_enabled && fmt == DownloadFormat::Mp4 {
        let lang = if let Some(ref custom) = app.preview_subs_custom_lang {
            custom.clone()
        } else if let PreviewState::Ready { ref info } = app.preview_state {
            info.subtitle_langs
                .get(app.preview_subs_lang_cursor)
                .cloned()
                .unwrap_or_else(|| "en".to_string())
        } else {
            "en".to_string()
        };
        Some(SubtitleOptions { lang })
    } else {
        None
    };

    let mut s = app.settings.clone();
    if !quality_str.is_empty() {
        s.video_quality = quality_str;
    }
    if let Some(ref c) = app.cookies_file {
        s.ytdlp_cookies = c.clone();
    }

    let mut thumb_url = None;
    if let PreviewState::Ready { info, .. } = &app.preview_state {
        thumb_url = info.thumbnail_url.clone();
    }

    let id = app.add_download(url.clone(), fmt);
    if let Some(slot) = app.slot_mut(id) {
        slot.task_spawned = true;
        slot.thumbnail_url = thumb_url;
    }
    let handle = spawn_download(id, url, fmt, s, dl_tx, subtitle_opts);
    if let Some(slot) = app.slot_mut(id) {
        slot.cancel = Some(handle);
    }

    // Reset preview + subtitle state
    app.preview_state = PreviewState::Hidden;
    app.preview_thumbnail = None;
    app.preview_image_protocol = None;
    app.preview_subs_menu = false;
    app.preview_subs_enabled = false;
    app.preview_subs_lang_cursor = 0;
    app.preview_subs_custom_lang = None;
    app.preview_subs_editing = false;
    app.preview_subs_edit_buf.clear();
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
    if !std::path::Path::new(&path).exists() {
        app.add_toast("File not found", ToastKind::Error);
        return;
    }
    #[cfg(target_os = "macos")]
    {
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
