//! Root UI renderer for dora TUI.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Tabs};
use ratatui::Frame;

use crate::app::{App, ClickTarget, HistoryEntry, Particle, SlotState, ToastKind, YtdlpStartup};
use crate::theme::ThemeColors;

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
    // slot_screen_rects is cleared inside render_downloads each frame.

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
    let theme = app.theme;
    if app.help_visible {
        render_help_overlay(f, size, &theme);
    }
    if app.show_cookies_input {
        render_cookies_popup(f, size, app);
    }
    if let Some(path) = app.reveal_popup.clone() {
        render_reveal_popup(f, size, &path, &theme);
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
    let blue = app.theme.blue;
    let base = app.theme.base;
    match &app.ytdlp_startup.clone() {
        YtdlpStartup::Missing => render_ytdlp_missing_popup(f, size, &theme),
        YtdlpStartup::Updating { msg } => render_ytdlp_updating_popup(f, size, msg, 1.0, blue, base),
        YtdlpStartup::FadingOut { ticks } => {
            let alpha = *ticks as f32 / 90.0;
            render_ytdlp_updating_popup(f, size, "  ✓  Up to date", alpha, blue, base);
        }
        YtdlpStartup::Done => {}
    }

    // Feature: TUI Toasts
    render_toasts(f, size, app);

    // Supernova particles rendered last — on top of everything.
    render_particles(f, &app.particles, f.area());
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Combined Downloads tab: queue panel on the left, history on the right.
fn render_downloads_combined(f: &mut Frame, area: Rect, app: &mut App) {
    // Feature: Responsive UI
    if area.width < 100 {
        // Narrow: only show queue
        queue::render_queue(f, area, app);
    } else {
        // Wide: show both
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
                Style::default().fg(app.theme.lavender).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(app.theme.subtext)
            };
            Line::from(Span::styled(t.label(), style))
        })
        .collect();

    // Draw the bottom border spanning the full width
    let border_block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(app.theme.surface0));
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
        .highlight_style(Style::default().fg(app.theme.lavender).add_modifier(Modifier::BOLD))
        .divider(Span::styled("  │  ", Style::default().fg(app.theme.surface0)));

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
    let mut badge_spans: Vec<Span> = Vec::new();
    if app.ytdlp_available {
        badge_spans.push(Span::styled(
            "▶ yt",
            Style::default()
                .fg(ratatui::style::Color::Red)
                .add_modifier(Modifier::BOLD),
        ));
        badge_spans.push(Span::styled("−dlp", Style::default().fg(ratatui::style::Color::White)));
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
        let badge_y = area.y + area.height.saturating_sub(2);
        if badge_x > centered_area.x + content_w {
            let badge_area = Rect::new(badge_x, badge_y, badges_w, 1);
            f.render_widget(Paragraph::new(Line::from(badge_spans)), badge_area);
        }
    }
}

/// Package version sourced directly from Cargo.toml at compile time.
const VERSION: &str = env!("CARGO_PKG_VERSION");

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

    // Append active theme flavour to version indicator
    let flavour_label = app.settings.theme_flavour.label();
    let right_str = match (active, pending) {
        (0, 0) => format!(" {}  v{} [{}] ", now, VERSION, flavour_label),
        (a, 0) => format!(" ↓ {} downloading  {}  v{} [{}] ", a, now, VERSION, flavour_label),
        (0, p) => format!(" ⏳ {} pending  {}  v{} [{}] ", p, now, VERSION, flavour_label),
        (a, p) => format!(" ↓ {}  ⏳ {}  {}  v{} [{}] ", a, p, now, VERSION, flavour_label),
    };
    let right_width = right_str.chars().count() as u16;

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(1), Constraint::Length(right_width)])
        .split(area);

    // Left side: Bottom Keybar (htop-style)
    let k = |key: &'static str, label: &'static str| {
        vec![
            Span::styled(
                format!(" {} ", key),
                Style::default()
                    .bg(app.theme.surface0)
                    .fg(app.theme.lavender)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {} ", label), Style::default().fg(app.theme.subtext)),
        ]
    };

    let mut hints = Vec::new();
    hints.extend(k("1-3", "Tabs"));
    hints.extend(k("Enter", "Preview"));
    hints.extend(k("r", "Reveal"));
    hints.extend(k("d", "Delete"));
    hints.extend(k("T", "Theme"));
    hints.extend(k("?", "Help"));
    hints.extend(k("^C", "Quit"));

    f.render_widget(Paragraph::new(Line::from(hints)), chunks[0]);

    f.render_widget(
        Paragraph::new(right_str)
            .style(Style::default().fg(app.theme.lavender))
            .alignment(Alignment::Right),
        chunks[1],
    );
}

fn render_cookies_popup(f: &mut Frame, area: Rect, app: &App) {
    if area.width < 60 || area.height < 14 {
        return;
    }

    let popup_w = 72_u16.min(area.width.saturating_sub(4));
    let popup_h = 14_u16.min(area.height.saturating_sub(4));
    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

    let outer = Block::default()
        .title(" 🍪 Cookie File ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(app.theme.peach))
        .style(Style::default().bg(app.theme.base));
    let inner = outer.inner(popup_area);
    f.render_widget(Clear, popup_area);
    f.render_widget(outer, popup_area);

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

    let dz_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(app.theme.surface0))
        .style(Style::default().bg(app.theme.base));
    let dz_inner = dz_block.inner(rows[1]);
    f.render_widget(dz_block, rows[1]);

    let dz_content: Vec<Line> = vec![
        Line::from(""),
        Line::from(Span::styled(
            "   Drag & drop your cookies.txt file into this window",
            Style::default().fg(app.theme.subtext),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("        or press  ", Style::default().fg(app.theme.subtext)),
            Span::styled("[o]", Style::default().fg(app.theme.peach).add_modifier(Modifier::BOLD)),
            Span::styled("  to open the file browser", Style::default().fg(app.theme.subtext)),
        ]),
    ];
    f.render_widget(Paragraph::new(dz_content), dz_inner);

    let cursor = if app.blink_on { "│" } else { " " };
    let input_text = format!("  Path ❯ {}{}  ", app.cookies_input, cursor);
    f.render_widget(
        Paragraph::new(input_text).style(Style::default().fg(app.theme.text)),
        rows[3],
    );

    let status_text = match &app.cookies_file {
        Some(cf) => format!("  ✓ Set: {}", cf),
        None => "  Not set — yt-dlp runs without cookies (Tier 1 mode)".to_string(),
    };
    let status_color = if app.cookies_file.is_some() {
        app.theme.green
    } else {
        app.theme.subtext
    };
    f.render_widget(
        Paragraph::new(status_text).style(Style::default().fg(status_color)),
        rows[4],
    );

    let k = |s: &'static str| Span::styled(s, Style::default().fg(app.theme.peach).add_modifier(Modifier::BOLD));
    let sep = || Span::styled("  ", Style::default());
    let d = |s: &'static str| Span::styled(s, Style::default().fg(app.theme.subtext));
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

fn render_reveal_popup(f: &mut Frame, area: Rect, path: &str, theme: &ThemeColors) {
    let popup_w = 80_u16.min(area.width.saturating_sub(4));
    let popup_h = 7_u16.min(area.height.saturating_sub(4));
    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

    let block = Block::default()
        .title(" 📁 File Location ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.green))
        .style(Style::default().bg(theme.base));
    let inner = block.inner(popup_area);

    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}", path),
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "                    Press any key to close",
            Style::default().fg(theme.subtext),
        )),
    ];

    f.render_widget(Clear, popup_area);
    f.render_widget(block, popup_area);
    f.render_widget(Paragraph::new(text), inner);
}

fn render_help_overlay(f: &mut Frame, area: Rect, theme: &ThemeColors) {
    if area.width < 44 || area.height < 14 {
        return;
    }

    let popup_w = 60_u16.min(area.width.saturating_sub(4));
    let popup_h = 25_u16.min(area.height.saturating_sub(4));
    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

    let block = Block::default()
        .title(" ? Keyboard Shortcuts ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.lavender))
        .style(Style::default().bg(theme.base));

    let k = |s: &'static str| Span::styled(s, Style::default().fg(theme.peach).add_modifier(Modifier::BOLD));
    let d = |s: &'static str| Span::styled(s, Style::default().fg(theme.text));
    let h = |s: &'static str| {
        Span::styled(
            s,
            Style::default()
                .fg(theme.lavender)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )
    };
    let dim = |s: &'static str| Span::styled(s, Style::default().fg(theme.subtext));

    let text: Vec<Line> = vec![
        Line::from(""),
        Line::from(h("  Global")),
        Line::from(vec![
            k("  1/2/3           "),
            d("Switch tabs (Downloads / Lyrics / Settings)"),
        ]),
        Line::from(vec![
            k("  T               "),
            d("Cycle Catppuccin theme (Mocha/Macchiato/Frappe/Latte)"),
        ]),
        Line::from(vec![k("  Space           "), d("Toggle selection in History")]),
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

fn render_ytdlp_missing_popup(f: &mut Frame, area: Rect, theme: &ThemeColors) {
    let popup_w = 62_u16.min(area.width.saturating_sub(4));
    let popup_h = 9_u16.min(area.height.saturating_sub(4));
    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

    let block = Block::default()
        .title(" ⚠  yt-dlp not found ")
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(theme.red))
        .style(Style::default().bg(theme.base));
    let inner = block.inner(popup_area);

    let k = |s: &'static str| Span::styled(s, Style::default().fg(theme.peach).add_modifier(Modifier::BOLD));
    let d = |s: &'static str| Span::styled(s, Style::default().fg(theme.text));

    let text: Vec<Line> = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  yt-dlp is required for dora to download media.",
            Style::default().fg(theme.subtext),
        )),
        Line::from(Span::styled(
            "  Install it with:  pip install yt-dlp  or  brew install yt-dlp",
            Style::default().fg(theme.subtext),
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

#[allow(clippy::too_many_arguments)]
fn render_ytdlp_updating_popup(f: &mut Frame, area: Rect, msg: &str, alpha: f32, blue: Color, base: Color) {
    let popup_w = 58_u16.min(area.width.saturating_sub(4));
    let popup_h = 6_u16.min(area.height.saturating_sub(4));
    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + area.height.saturating_sub(popup_h + 2);
    let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

    // Fade BLUE → BASE as alpha goes 1.0 → 0.0.
    let color = lerp_color_rgb(blue, base, 1.0 - alpha);

    let block = Block::default()
        .title(" ↑  yt-dlp ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .style(Style::default().bg(base));
    let inner = block.inner(popup_area);

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

fn render_history_popup(f: &mut Frame, area: Rect, entry: &HistoryEntry, app: &mut App) {
    let thumb = app.preview_thumbnail.as_ref();
    let is_vertical = thumb.is_some_and(|t| (t.height * 2) > t.width);

    let (mut popup_w, mut popup_h) = (74_u16, 20_u16);
    if is_vertical {
        popup_w = 64;
        popup_h = 24;
    } else if thumb.is_some() {
        popup_w = 80;
        popup_h = 30;
    }

    let popup_w = popup_w.min(area.width.saturating_sub(4));
    let popup_h = popup_h.min(area.height.saturating_sub(4));
    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

    // Feature: Close on click (Background blocker)
    app.click_map.push((area, ClickTarget::PreviewClose));

    let is_mp4 = entry.format == crate::app::DownloadFormat::Mp4;
    let fmt_color = if is_mp4 { app.theme.blue } else { app.theme.peach };

    let title_trunc: String = entry.title.chars().take(50).collect();
    let block = Block::default()
        .title(format!(" {} ", title_trunc))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(app.theme.lavender))
        .style(Style::default().bg(app.theme.base));

    let inner = block.inner(popup_area);
    f.render_widget(Clear, popup_area);
    f.render_widget(block, popup_area);

    if is_vertical {
        // Shorts Layout: side-by-side
        let art_w = thumb.map(|t| t.width + 2).unwrap_or(26);
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(art_w), Constraint::Min(0)])
            .split(inner);

        preview::render_thumbnail(f, chunks[0], thumb, &app.theme, app);
        render_history_details(f, chunks[1], entry, app, fmt_color);
    } else {
        // Video Layout: top-bottom
        let art_h = thumb.map(|t| t.height + 1).unwrap_or(10);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(art_h), Constraint::Min(0)])
            .split(inner);

        preview::render_thumbnail(f, chunks[0], thumb, &app.theme, app);
        render_history_details(f, chunks[1], entry, app, fmt_color);
    }
}

fn render_history_details(f: &mut Frame, area: Rect, entry: &HistoryEntry, app: &mut App, fmt_color: Color) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // info rows
            Constraint::Length(3), // action buttons
        ])
        .split(area);

    let max_info = chunks[0].width.saturating_sub(3) as usize;
    let trunc = |s: &str| -> String { s.chars().take(max_info).collect() };

    let when = entry.finished_at.format("%H:%M  %d %b %Y").to_string();

    let info_lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(Span::styled(
            trunc(&entry.title),
            Style::default().fg(app.theme.text).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            trunc(&entry.artist),
            Style::default().fg(app.theme.subtext),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Format  ", Style::default().fg(app.theme.subtext)),
            Span::styled(
                entry.format.label(),
                Style::default().fg(fmt_color).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("Size    ", Style::default().fg(app.theme.subtext)),
            Span::styled(format!("{:.1} MB", entry.size_mb), Style::default().fg(app.theme.blue)),
        ]),
        Line::from(vec![
            Span::styled("Date    ", Style::default().fg(app.theme.subtext)),
            Span::styled(when, Style::default().fg(app.theme.lavender)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Path  ", Style::default().fg(app.theme.subtext)),
            Span::styled(trunc(&entry.path), Style::default().fg(app.theme.text)),
        ]),
    ];
    f.render_widget(Paragraph::new(info_lines), chunks[0]);

    let k = |s: &'static str| Span::styled(s, Style::default().fg(app.theme.peach).add_modifier(Modifier::BOLD));
    let d = |s: &'static str| Span::styled(s, Style::default().fg(app.theme.subtext));
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
        .border_style(Style::default().fg(app.theme.surface0));
    let btn_area = chunks[1];
    let btn_inner = btn_block.inner(btn_area);
    f.render_widget(btn_block, btn_area);
    f.render_widget(Paragraph::new(Line::from(btn_spans)), btn_inner);

    // Register click targets for the button row
    let btn_w = btn_inner.width / 4;
    app.click_map.push((
        Rect::new(btn_inner.x, btn_inner.y, btn_w, btn_inner.height),
        ClickTarget::HistoryReveal(entry.path.clone()),
    ));
    if !entry.url.is_empty() {
        app.click_map.push((
            Rect::new(btn_inner.x + btn_w, btn_inner.y, btn_w, btn_inner.height),
            ClickTarget::OpenInBrowser(entry.url.clone()),
        ));
    }
    app.click_map.push((
        Rect::new(btn_inner.x + btn_w * 2, btn_inner.y, btn_w, btn_inner.height),
        // We don't have a direct "Delete" target index here easily,
        // but we can use the close target for now or just wait for keypress.
        // Better: let's just make the "Close" part work.
        ClickTarget::PreviewClose,
    ));
    app.click_map.push((
        Rect::new(
            btn_inner.x + btn_inner.width.saturating_sub(btn_w),
            btn_inner.y,
            btn_w,
            btn_inner.height,
        ),
        ClickTarget::PreviewClose,
    ));
}

// ── Toast System ──────────────────────────────────────────────────────────────

fn render_toasts(f: &mut Frame, size: Rect, app: &App) {
    if app.toasts.is_empty() {
        return;
    }

    let toast_w = 32_u16;
    let toast_h = 3_u16;
    let mut current_y = size.y + 1; // Top-right corner

    for toast in app.toasts.iter().rev() {
        let x = size.x + size.width.saturating_sub(toast_w + 1);
        let area = Rect::new(x, current_y, toast_w, toast_h);

        let (border_color, icon) = match toast.kind {
            ToastKind::Info => (app.theme.blue, "ℹ "),
            ToastKind::Success => (app.theme.green, "✔ "),
            ToastKind::Error => (app.theme.red, "✖ "),
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color))
            .style(Style::default().bg(app.theme.crust));

        let msg = Paragraph::new(format!(" {}{}", icon, toast.message))
            .block(block)
            .style(Style::default().fg(app.theme.text));

        f.render_widget(Clear, area);
        f.render_widget(msg, area);

        current_y += toast_h;
        if current_y + toast_h > size.height {
            break;
        }
    }
}

// ── Supernova particle overlay ────────────────────────────────────────────────

fn render_particles(f: &mut Frame, particles: &[Particle], bounds: Rect) {
    for p in particles {
        let x = p.x as u16;
        let y = p.y as u16;
        // Clamp to terminal bounds.
        if x < bounds.x || y < bounds.y || x >= bounds.x + bounds.width || y >= bounds.y + bounds.height {
            continue;
        }
        // Fade out over the last 40% of life.
        let t = p.age / p.max_age;
        if t >= 1.0 {
            continue;
        }
        let cell_area = Rect::new(x, y, 1, 1);
        f.render_widget(
            Paragraph::new(p.ch.to_string()).style(Style::default().fg(p.color).add_modifier(Modifier::BOLD)),
            cell_area,
        );
    }
}

// ── Color helpers ─────────────────────────────────────────────────────────────

/// Linearly interpolate between two Color::Rgb values.
fn lerp_color_rgb(from: Color, to: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    fn rgb(c: Color) -> (u8, u8, u8) {
        match c {
            Color::Rgb(r, g, b) => (r, g, b),
            _ => (128, 128, 128),
        }
    }
    let (fr, fg, fb) = rgb(from);
    let (tr, tg, tb) = rgb(to);
    Color::Rgb(
        (fr as f32 * (1.0 - t) + tr as f32 * t) as u8,
        (fg as f32 * (1.0 - t) + tg as f32 * t) as u8,
        (fb as f32 * (1.0 - t) + tb as f32 * t) as u8,
    )
}
