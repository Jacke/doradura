//! Root UI renderer for dora TUI.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Tabs};
use ratatui::Frame;

use crate::app::{App, ClickTarget, HistoryEntry, SlotState, YtdlpStartup};
use crate::theme;

mod history;
mod logo;
mod lyrics;
pub mod preview;
mod queue;
pub mod settings;

/// Render the entire TUI for the current frame.
pub fn render(f: &mut Frame, app: &mut App) {
    let size = f.area();

    // Clear click map for this frame
    app.click_map.clear();

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8), // logo
            Constraint::Length(3), // tab bar
            Constraint::Min(1),    // main content
            Constraint::Length(1), // status bar
        ])
        .split(size);

    logo::render_logo(f, vertical[0], app);
    render_tabs(f, vertical[1], app);

    match app.active_tab {
        crate::app::Tab::Downloads => render_downloads_combined(f, vertical[2], app),
        crate::app::Tab::Lyrics => lyrics::render_lyrics(f, vertical[2], app),
        crate::app::Tab::Settings => settings::render_settings(f, vertical[2], app),
    }

    render_status_bar(f, vertical[3], app);

    // Overlays rendered last so they appear on top of everything.
    if app.help_visible {
        render_help_overlay(f, size);
    }
    if app.show_cookies_input {
        render_cookies_popup(f, size, app);
    }
    if let Some(path) = app.reveal_popup.clone() {
        render_reveal_popup(f, size, &path);
    }
    if app.preview_state.is_visible() {
        preview::render_preview_popup(f, size, app);
    }
    if let Some(idx) = app.history_popup {
        if let Some(entry) = app.history.iter().rev().nth(idx).cloned() {
            render_history_popup(f, size, &entry, app);
        }
    }

    // yt-dlp startup popups render on top of everything (highest z-order).
    match &app.ytdlp_startup.clone() {
        YtdlpStartup::Missing => render_ytdlp_missing_popup(f, size),
        YtdlpStartup::Updating { msg } => render_ytdlp_updating_popup(f, size, msg, 1.0),
        YtdlpStartup::FadingOut { ticks } => {
            let alpha = *ticks as f32 / 90.0;
            render_ytdlp_updating_popup(f, size, "  ✓  Up to date", alpha);
        }
        YtdlpStartup::Done => {}
    }
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Combined Downloads tab: queue panel on the left, history on the right.
fn render_downloads_combined(f: &mut Frame, area: Rect, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(46), // queue panel (fixed width)
            Constraint::Min(0),     // history panel (remaining width)
        ])
        .split(area);

    queue::render_queue(f, chunks[0], app);
    history::render_history(f, chunks[1], app);
}

fn render_tabs(f: &mut Frame, area: Rect, app: &mut App) {
    let tabs_list = [
        crate::app::Tab::Downloads,
        crate::app::Tab::Lyrics,
        crate::app::Tab::Settings,
    ];

    let titles: Vec<Line> = tabs_list
        .iter()
        .map(|t| {
            let style = if *t == app.active_tab {
                Style::default().fg(theme::LAVENDER).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::SUBTEXT)
            };
            Line::from(Span::styled(t.label(), style))
        })
        .collect();

    // Draw the bottom border spanning the full width
    let border_block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(theme::SURFACE0));
    f.render_widget(border_block, area);

    // Compute approximate tab-bar content width so we can center it.
    let n = tabs_list.len();
    let label_chars: usize = tabs_list.iter().map(|t| t.label().chars().count()).sum();
    let divider_chars = 5 * (n.saturating_sub(1));
    let padding_chars = 2 * n;
    let content_w = (label_chars + divider_chars + padding_chars) as u16;

    // Centre the tabs widget inside the area
    let centered_area = if content_w < area.width {
        let x_offset = (area.width.saturating_sub(content_w)) / 2;
        Rect::new(area.x + x_offset, area.y, content_w, area.height)
    } else {
        area
    };

    let tabs = Tabs::new(titles)
        .select(app.active_tab.index())
        .highlight_style(Style::default().fg(theme::LAVENDER).add_modifier(Modifier::BOLD))
        .divider(Span::styled("  │  ", Style::default().fg(theme::SURFACE0)));

    f.render_widget(tabs, centered_area);

    // Register each tab's clickable area in the app's click map
    {
        let mut x = centered_area.x;
        for (i, tab) in tabs_list.iter().enumerate() {
            let label_w = tab.label().chars().count() as u16 + 2;
            let click_rect = Rect::new(x, centered_area.y, label_w, centered_area.height);
            app.click_map.push((click_rect, ClickTarget::SwitchTab(*tab)));
            x += label_w;
            if i < tabs_list.len() - 1 {
                x += 5; // "  │  " divider
            }
        }
    }

    // ── Plugin availability badges (right side of tab bar) ──────────────────
    // Show mini icons for configured/available integrations.
    let mut badge_spans: Vec<Span> = Vec::new();
    if app.ytdlp_available {
        badge_spans.push(Span::styled(
            "▶ yt",
            Style::default()
                .fg(ratatui::style::Color::Red)
                .add_modifier(Modifier::BOLD),
        ));
        badge_spans.push(Span::styled("-dlp", Style::default().fg(ratatui::style::Color::White)));
    }
    let ig_active = !app.settings.instagram_cookies.is_empty() || !app.settings.instagram_doc_id.is_empty();
    if ig_active {
        if !badge_spans.is_empty() {
            badge_spans.push(Span::raw("  "));
        }
        badge_spans.push(Span::styled(
            "◉",
            Style::default()
                .fg(ratatui::style::Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ));
        badge_spans.push(Span::styled(
            " Instagram",
            Style::default().fg(ratatui::style::Color::LightMagenta),
        ));
    }
    if !badge_spans.is_empty() {
        badge_spans.push(Span::raw(" "));
        let badges_w: u16 = badge_spans.iter().map(|s| s.content.chars().count() as u16).sum();
        let badge_x = area.x + area.width.saturating_sub(badges_w);
        let badge_y = area.y + area.height.saturating_sub(2); // above the bottom border
        if badge_x > centered_area.x + content_w {
            let badge_area = Rect::new(badge_x, badge_y, badges_w, 1);
            f.render_widget(Paragraph::new(Line::from(badge_spans)), badge_area);
        }
    }
}

fn render_status_bar(f: &mut Frame, area: Rect, app: &App) {
    let now = chrono::Local::now().format("%H:%M").to_string();

    let active = app
        .slots
        .iter()
        .filter(|s| matches!(s.state, SlotState::Downloading { .. }))
        .count();
    let pending = app
        .slots
        .iter()
        .filter(|s| matches!(s.state, SlotState::Pending | SlotState::Fetching))
        .count();

    let right_str = match (active, pending) {
        (0, 0) => format!(" {}  v0.1 ", now),
        (a, 0) => format!(" ↓ {} downloading  {}  v0.1 ", a, now),
        (0, p) => format!(" ⏳ {} pending  {}  v0.1 ", p, now),
        (a, p) => format!(" ↓ {}  ⏳ {}  {}  v0.1 ", a, p, now),
    };
    let right_width = right_str.chars().count() as u16;

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(1), Constraint::Length(right_width)])
        .split(area);

    // Left side: error notification takes priority over the normal hint line.
    if let Some((msg, set_at)) = &app.status_msg {
        // Fade the red toward SUBTEXT in the last 3 seconds.
        let elapsed = set_at.elapsed().as_secs_f32();
        let color = if elapsed < 4.0 {
            theme::RED
        } else {
            // lerp RED → SUBTEXT over the final 2 s
            let t = ((elapsed - 4.0) / 2.0).clamp(0.0, 1.0);
            lerp_color((243, 139, 168), (166, 173, 200), 1.0 - t)
        };
        let max_chars = chunks[0].width.saturating_sub(1) as usize;
        let display: String = msg.chars().take(max_chars).collect();
        f.render_widget(
            Paragraph::new(display).style(Style::default().fg(color).add_modifier(Modifier::BOLD)),
            chunks[0],
        );
    } else {
        f.render_widget(
            Paragraph::new(" [1-3] Tabs  [Enter] Preview  [r] Reveal  [d] Delete  [?] Help  [Esc]  [Ctrl+C] Quit")
                .style(Style::default().fg(theme::SUBTEXT)),
            chunks[0],
        );
    }

    f.render_widget(
        Paragraph::new(right_str)
            .style(Style::default().fg(theme::LAVENDER))
            .alignment(Alignment::Right),
        chunks[1],
    );
}

fn render_cookies_popup(f: &mut Frame, area: Rect, app: &App) {
    // Need at least 14 rows × 60 cols to render meaningfully
    if area.width < 60 || area.height < 14 {
        return;
    }

    let popup_w = 72_u16.min(area.width.saturating_sub(4));
    let popup_h = 14_u16.min(area.height.saturating_sub(4));
    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

    // ── Outer popup frame ─────────────────────────────────────────────────────
    let outer = Block::default()
        .title(" 🍪 Cookie File ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::PEACH))
        .style(Style::default().bg(theme::BASE));
    let inner = outer.inner(popup_area);
    f.render_widget(Clear, popup_area);
    f.render_widget(outer, popup_area);

    // ── Inner layout: top-gap / drop-zone / gap / input / status / hints ──────
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // top gap
            Constraint::Length(6), // drop zone block
            Constraint::Length(1), // gap
            Constraint::Length(1), // path input
            Constraint::Length(1), // current status
            Constraint::Length(1), // keyboard hints
            Constraint::Length(1), // bottom gap
        ])
        .split(inner);

    // ── Drop zone ─────────────────────────────────────────────────────────────
    let dz_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(theme::SURFACE0))
        .style(Style::default().bg(theme::BASE));
    let dz_inner = dz_block.inner(rows[1]);
    f.render_widget(dz_block, rows[1]);

    let dz_content: Vec<Line> = vec![
        Line::from(""),
        Line::from(Span::styled(
            "   Drag & drop your cookies.txt file into this window",
            Style::default().fg(theme::SUBTEXT),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("        or press  ", Style::default().fg(theme::SUBTEXT)),
            Span::styled("[o]", Style::default().fg(theme::PEACH).add_modifier(Modifier::BOLD)),
            Span::styled("  to open the file browser", Style::default().fg(theme::SUBTEXT)),
        ]),
    ];
    f.render_widget(Paragraph::new(dz_content), dz_inner);

    // ── Path input ────────────────────────────────────────────────────────────
    let cursor = if app.spinner_frame % 60 < 30 { "│" } else { " " };
    let input_text = format!("  Path ❯ {}{}  ", app.cookies_input, cursor);
    f.render_widget(
        Paragraph::new(input_text).style(Style::default().fg(theme::TEXT)),
        rows[3],
    );

    // ── Current status ────────────────────────────────────────────────────────
    let status_text = match &app.cookies_file {
        Some(cf) => format!("  ✓ Set: {}", cf),
        None => "  Not set — yt-dlp runs without cookies (Tier 1 mode)".to_string(),
    };
    let status_color = if app.cookies_file.is_some() {
        theme::GREEN
    } else {
        theme::SUBTEXT
    };
    f.render_widget(
        Paragraph::new(status_text).style(Style::default().fg(status_color)),
        rows[4],
    );

    // ── Keyboard hints ────────────────────────────────────────────────────────
    let k = |s: &'static str| Span::styled(s, Style::default().fg(theme::PEACH).add_modifier(Modifier::BOLD));
    let sep = || Span::styled("  ", Style::default());
    let d = |s: &'static str| Span::styled(s, Style::default().fg(theme::SUBTEXT));
    let hints = Line::from(vec![
        k("  [Enter]"),
        d(" Confirm"),
        sep(),
        k("[o]"),
        d(" Browse"),
        sep(),
        k("[Del]"),
        d(" Clear"),
        sep(),
        k("[Esc]"),
        d(" Cancel"),
    ]);
    f.render_widget(Paragraph::new(hints), rows[5]);
}

fn render_reveal_popup(f: &mut Frame, area: Rect, path: &str) {
    let popup_w = 80_u16.min(area.width.saturating_sub(4));
    let popup_h = 7_u16.min(area.height.saturating_sub(4));
    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

    let block = Block::default()
        .title(" 📁 File Location ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::GREEN))
        .style(Style::default().bg(theme::BASE));
    let inner = block.inner(popup_area);

    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}", path),
            Style::default().fg(theme::TEXT).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "                    Press any key to close",
            Style::default().fg(theme::SUBTEXT),
        )),
    ];

    f.render_widget(Clear, popup_area);
    f.render_widget(block, popup_area);
    f.render_widget(Paragraph::new(text), inner);
}

fn render_help_overlay(f: &mut Frame, area: Rect) {
    if area.width < 44 || area.height < 14 {
        return; // terminal too small to render help meaningfully
    }

    let popup_w = 60_u16.min(area.width.saturating_sub(4));
    let popup_h = 23_u16.min(area.height.saturating_sub(4));
    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

    let block = Block::default()
        .title(" ? Keyboard Shortcuts ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::LAVENDER))
        .style(Style::default().bg(theme::BASE));

    // Helpers for styling key labels and descriptions
    let k = |s: &'static str| Span::styled(s, Style::default().fg(theme::PEACH).add_modifier(Modifier::BOLD));
    let d = |s: &'static str| Span::styled(s, Style::default().fg(theme::TEXT));
    let h = |s: &'static str| {
        Span::styled(
            s,
            Style::default()
                .fg(theme::LAVENDER)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )
    };
    let dim = |s: &'static str| Span::styled(s, Style::default().fg(theme::SUBTEXT));

    let text: Vec<Line> = vec![
        Line::from(""),
        Line::from(h("  Global")),
        Line::from(vec![
            k("  1/2/3           "),
            d("Switch tabs (Downloads / Lyrics / Settings)"),
        ]),
        Line::from(vec![k("  ?               "), d("Open this help overlay")]),
        Line::from(vec![k("  Esc             "), d("Close popup  /  clear active input")]),
        Line::from(vec![k("  Ctrl+C          "), d("Quit")]),
        Line::from(""),
        Line::from(h("  Downloads Tab")),
        Line::from(vec![k("  Enter           "), d("Open preview → then confirm download")]),
        Line::from(vec![
            k("  c               "),
            d("Set cookies file (authenticated sites)"),
        ]),
        Line::from(vec![k("  r               "), d("Reveal most recent finished file")]),
        Line::from(vec![k("  d               "), d("Remove last finished / failed slot")]),
        Line::from(""),
        Line::from(h("  Preview Popup")),
        Line::from(vec![k("  ← →            "), d("Select quality (MP4)")]),
        Line::from(vec![k("  Tab             "), d("Toggle MP3 / MP4 in preview")]),
        Line::from(vec![k("  Enter           "), d("Start download with selected options")]),
        Line::from(vec![k("  Esc             "), d("Cancel preview, restore URL")]),
        Line::from(""),
        Line::from(vec![
            k("  ↑ / ↓           "),
            d("Scroll history (when URL bar is empty)"),
        ]),
        Line::from(vec![
            k("  r / Enter       "),
            d("Reveal selected history file in Finder"),
        ]),
        Line::from(""),
        Line::from(h("  Lyrics Tab")),
        Line::from(vec![k("  Enter           "), d("Search for lyrics")]),
        Line::from(vec![k("  ↑ / ↓           "), d("Scroll lyrics")]),
        Line::from(""),
        Line::from(h("  Settings Tab")),
        Line::from(vec![k("  ↑ / ↓           "), d("Navigate settings items")]),
        Line::from(vec![k("  ← / →           "), d("Cycle option values")]),
        Line::from(vec![k("  Enter           "), d("Edit text field")]),
        Line::from(vec![k("  o               "), d("Browse for file path")]),
        Line::from(vec![k("  s               "), d("Save settings to disk")]),
        Line::from(""),
        Line::from(dim("                    Press any key to close")),
    ];

    let help = Paragraph::new(text).block(block);
    f.render_widget(Clear, popup_area);
    f.render_widget(help, popup_area);
}

fn render_ytdlp_missing_popup(f: &mut Frame, area: Rect) {
    let popup_w = 62_u16.min(area.width.saturating_sub(4));
    let popup_h = 9_u16.min(area.height.saturating_sub(4));
    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

    let block = Block::default()
        .title(" ⚠  yt-dlp not found ")
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(theme::RED))
        .style(Style::default().bg(theme::BASE));
    let inner = block.inner(popup_area);

    let k = |s: &'static str| Span::styled(s, Style::default().fg(theme::PEACH).add_modifier(Modifier::BOLD));
    let d = |s: &'static str| Span::styled(s, Style::default().fg(theme::TEXT));

    let text: Vec<Line> = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  yt-dlp is required for dora to download media.",
            Style::default().fg(theme::SUBTEXT),
        )),
        Line::from(Span::styled(
            "  Install it with:  pip install yt-dlp  or  brew install yt-dlp",
            Style::default().fg(theme::SUBTEXT),
        )),
        Line::from(""),
        Line::from(vec![
            k("  [i]"),
            d(" Open yt-dlp releases in browser    "),
            k("[q] / [Esc]"),
            d(" Quit"),
        ]),
    ];

    f.render_widget(Clear, popup_area);
    f.render_widget(block, popup_area);
    f.render_widget(Paragraph::new(text), inner);
}

fn render_ytdlp_updating_popup(f: &mut Frame, area: Rect, msg: &str, alpha: f32) {
    let popup_w = 58_u16.min(area.width.saturating_sub(4));
    let popup_h = 6_u16.min(area.height.saturating_sub(4));
    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + area.height.saturating_sub(popup_h + 2); // bottom of screen
    let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

    // Lerp BLUE → BASE as alpha goes 1.0 → 0.0 (fade to invisible against background).
    let color = lerp_color((137, 180, 250), (30, 30, 46), alpha);

    let block = Block::default()
        .title(" ↑  yt-dlp ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .style(Style::default().bg(theme::BASE));
    let inner = block.inner(popup_area);

    // Truncate the message to fit in the popup width
    let max_chars = (popup_w.saturating_sub(4)) as usize;
    let display: String = msg.chars().take(max_chars).collect();

    let text: Vec<Line> = vec![
        Line::from(""),
        Line::from(Span::styled(format!("  {}", display), Style::default().fg(color))),
    ];

    f.render_widget(Clear, popup_area);
    f.render_widget(block, popup_area);
    f.render_widget(Paragraph::new(text), inner);
}

/// Linearly interpolate between two RGB colours.
/// `t = 1.0` → `from`, `t = 0.0` → `to`.
fn lerp_color(from: (u8, u8, u8), to: (u8, u8, u8), t: f32) -> ratatui::style::Color {
    let t = t.clamp(0.0, 1.0);
    let r = (from.0 as f32 * t + to.0 as f32 * (1.0 - t)) as u8;
    let g = (from.1 as f32 * t + to.1 as f32 * (1.0 - t)) as u8;
    let b = (from.2 as f32 * t + to.2 as f32 * (1.0 - t)) as u8;
    ratatui::style::Color::Rgb(r, g, b)
}

fn render_history_popup(f: &mut Frame, area: Rect, entry: &HistoryEntry, app: &mut App) {
    let popup_w = 74_u16.min(area.width.saturating_sub(4));
    let popup_h = 20_u16.min(area.height.saturating_sub(4));
    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

    let is_mp4 = entry.format == crate::app::DownloadFormat::Mp4;
    let fmt_color = if is_mp4 { theme::BLUE } else { theme::PEACH };

    let title_trunc: String = entry.title.chars().take(50).collect();
    let block = Block::default()
        .title(format!(" {} ", title_trunc))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::LAVENDER))
        .style(Style::default().bg(theme::BASE));

    let inner = block.inner(popup_area);
    f.render_widget(Clear, popup_area);
    f.render_widget(block, popup_area);

    // ── Layout: art column (left) | info+buttons (right) ─────────────────────
    // art_w = 20: each art line is exactly 18 printable chars + 1 space margin right.
    // All chars used are unambiguously narrow (block elements, ASCII, box-drawing).
    let art_w: u16 = 20;
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(art_w), Constraint::Min(0)])
        .split(inner);

    // ── Left: format ASCII art (all single-width chars only) ──────────────────
    // Each string is exactly 18 chars wide (verified by comment count).
    let b = |s: &'static str, c| Span::styled(s, Style::default().fg(c));

    let art_lines: Vec<Line> = if is_mp4 {
        // MP4: film strip + play indicator
        // "  ╭────────────╮ " = 2+1+12+1+1+1 = 18? Let me count:
        // sp sp ╭ ────────────(12) ╮ sp = 2+1+12+1+1 = 17... need 18
        // "  ╭────────────╮  " = 2+1+12+1+2 = 18 ✓
        vec![
            Line::from(b("  ╭────────────╮  ", theme::BLUE)),
            Line::from(b("  │ [][][][][] │  ", theme::SURFACE0)),
            Line::from(b("  │            │  ", theme::BLUE)),
            Line::from(vec![
                b("  │     ", theme::BLUE),
                Span::styled(">>", Style::default().fg(theme::LAVENDER).add_modifier(Modifier::BOLD)),
                b("     │  ", theme::BLUE),
            ]),
            Line::from(b("  │            │  ", theme::BLUE)),
            Line::from(b("  │  VIDEO  MP4│  ", theme::BLUE)),
            Line::from(b("  │            │  ", theme::BLUE)),
            Line::from(b("  │ [][][][][] │  ", theme::SURFACE0)),
            Line::from(b("  ╰────────────╯  ", theme::BLUE)),
            Line::from(Span::raw("")),
            Line::from(Span::styled(
                "      [ MP4 ]     ",
                Style::default().fg(theme::BLUE).add_modifier(Modifier::BOLD),
            )),
        ]
    } else {
        // MP3: audio waveform using block elements (all narrow, guaranteed 1-wide)
        vec![
            Line::from(b("  ╭────────────╮  ", theme::PEACH)),
            Line::from(b("  │            │  ", theme::PEACH)),
            Line::from(b("  │  ▁▂▄▆▄▂▁▂  │  ", theme::GREEN)),
            Line::from(b("  │  ▂▄▇█▇▄▂▄  │  ", theme::GREEN)),
            Line::from(b("  │  ▃▆██▇▅▃▅  │  ", theme::GREEN)),
            Line::from(b("  │  ▁▃▆▇▆▃▁▃  │  ", theme::GREEN)),
            Line::from(b("  │  ▁▂▄▅▄▂▁▁  │  ", theme::GREEN)),
            Line::from(b("  │            │  ", theme::PEACH)),
            Line::from(b("  ╰────────────╯  ", theme::PEACH)),
            Line::from(Span::raw("")),
            Line::from(Span::styled(
                "      [ MP3 ]     ",
                Style::default().fg(theme::PEACH).add_modifier(Modifier::BOLD),
            )),
        ]
    };
    f.render_widget(Paragraph::new(art_lines), chunks[0]);

    // Clicking anywhere on the ASCII art column triggers "reveal in Finder".
    if !entry.path.is_empty() {
        app.click_map
            .push((chunks[0], ClickTarget::HistoryReveal(entry.path.clone())));
    }

    // ── Right: info + actions ─────────────────────────────────────────────────
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),    // info rows
            Constraint::Length(3), // action buttons
        ])
        .split(chunks[1]);

    let max_info = right[0].width.saturating_sub(3) as usize;
    let trunc = |s: &str| -> String { s.chars().take(max_info).collect() };

    let when = entry.finished_at.format("%H:%M  %d %b %Y").to_string();

    let info_lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(Span::styled(
            trunc(&entry.title),
            Style::default().fg(theme::TEXT).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(trunc(&entry.artist), Style::default().fg(theme::SUBTEXT))),
        Line::from(""),
        Line::from(vec![
            Span::styled("Format  ", Style::default().fg(theme::SUBTEXT)),
            Span::styled(
                entry.format.label(),
                Style::default().fg(fmt_color).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("Size    ", Style::default().fg(theme::SUBTEXT)),
            Span::styled(format!("{:.1} MB", entry.size_mb), Style::default().fg(theme::BLUE)),
        ]),
        Line::from(vec![
            Span::styled("Date    ", Style::default().fg(theme::SUBTEXT)),
            Span::styled(when, Style::default().fg(theme::LAVENDER)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Path  ", Style::default().fg(theme::SUBTEXT)),
            Span::styled(trunc(&entry.path), Style::default().fg(theme::TEXT)),
        ]),
    ];
    f.render_widget(Paragraph::new(info_lines), right[0]);

    // ── Action buttons ─────────────────────────────────────────────────────────
    let k = |s: &'static str| Span::styled(s, Style::default().fg(theme::PEACH).add_modifier(Modifier::BOLD));
    let d = |s: &'static str| Span::styled(s, Style::default().fg(theme::SUBTEXT));
    let sep = || Span::raw("   ");

    let mut btn_spans = vec![k(" [r] "), d("Reveal"), sep()];
    if !entry.url.is_empty() {
        btn_spans.push(k("[b] "));
        btn_spans.push(d("Browser"));
        btn_spans.push(sep());
    }
    btn_spans.push(k("[d] "));
    btn_spans.push(d("Delete"));
    btn_spans.push(sep());
    btn_spans.push(k("[Esc] "));
    btn_spans.push(d("Close"));

    let btn_block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme::SURFACE0));
    let btn_inner = btn_block.inner(right[1]);
    f.render_widget(btn_block, right[1]);
    f.render_widget(Paragraph::new(Line::from(btn_spans)), btn_inner);
}
