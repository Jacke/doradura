//! Root UI renderer for dora TUI.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Tabs, Wrap};
use ratatui::Frame;

use crate::app::{App, ClickTarget, SlotState};
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
        crate::app::Tab::Queue => queue::render_queue(f, vertical[2], app),
        crate::app::Tab::History => history::render_history(f, vertical[2], app),
        crate::app::Tab::Lyrics => lyrics::render_lyrics(f, vertical[2], app),
        crate::app::Tab::Settings => settings::render_settings(f, vertical[2], app),
    }

    render_status_bar(f, vertical[3], app);

    // Overlays rendered last so they appear on top of everything.
    if let Some(msg) = app.error_popup.clone() {
        render_error_popup(f, size, &msg);
    }
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
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn render_tabs(f: &mut Frame, area: Rect, app: &mut App) {
    let titles: Vec<Line> = [
        crate::app::Tab::Queue,
        crate::app::Tab::History,
        crate::app::Tab::Lyrics,
        crate::app::Tab::Settings,
    ]
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
    // Labels (visible chars) + dividers (5 chars each between tabs) + 2-char padding each side.
    let tab_labels = [
        crate::app::Tab::Queue.label(),
        crate::app::Tab::History.label(),
        crate::app::Tab::Lyrics.label(),
        crate::app::Tab::Settings.label(),
    ];
    let n = tab_labels.len();
    // Each tab: 1 padding_left + label_chars + 1 padding_right; dividers between tabs: "  │  " = 5 chars
    let label_chars: usize = tab_labels.iter().map(|l| l.chars().count()).sum();
    let divider_chars = 5 * (n.saturating_sub(1));
    let padding_chars = 2 * n; // default 1 char each side per tab
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
        let tabs_list = [
            crate::app::Tab::Queue,
            crate::app::Tab::History,
            crate::app::Tab::Lyrics,
            crate::app::Tab::Settings,
        ];
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

    let left = Paragraph::new(" [1-4] Tabs  [Enter] Preview  [r] Reveal  [d] Delete  [?] Help  [Esc]  [Ctrl+C] Quit")
        .style(Style::default().fg(theme::SUBTEXT));

    let right = Paragraph::new(right_str)
        .style(Style::default().fg(theme::LAVENDER))
        .alignment(Alignment::Right);

    f.render_widget(left, chunks[0]);
    f.render_widget(right, chunks[1]);
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

fn render_error_popup(f: &mut Frame, area: Rect, message: &str) {
    let popup_w = 60_u16.min(area.width.saturating_sub(4));
    let popup_h = 7_u16.min(area.height.saturating_sub(4));
    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

    let block = Block::default()
        .title(" ✖ Error ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::RED))
        .style(Style::default().bg(theme::BASE));

    let text = Paragraph::new(message)
        .block(block)
        .style(Style::default().fg(theme::TEXT).add_modifier(Modifier::BOLD))
        .wrap(Wrap { trim: true });

    f.render_widget(Clear, popup_area);
    f.render_widget(text, popup_area);
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
            k("  1/2/3/4         "),
            d("Switch tabs (Queue / History / Lyrics / Settings)"),
        ]),
        Line::from(vec![k("  ?               "), d("Open this help overlay")]),
        Line::from(vec![k("  Esc             "), d("Close popup  /  clear active input")]),
        Line::from(vec![k("  Ctrl+C          "), d("Quit")]),
        Line::from(""),
        Line::from(h("  Queue Tab")),
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
        Line::from(h("  History Tab")),
        Line::from(vec![k("  ↑ / ↓           "), d("Navigate history entries")]),
        Line::from(vec![k("  r / Enter       "), d("Reveal selected file in Finder")]),
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
