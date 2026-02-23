//! Queue tab: active downloads with progress gauges and URL input bar.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Gauge, Paragraph};
use ratatui::Frame;

use crate::app::{App, SlotState};
use crate::theme;

const SPINNER: &[&str] = &[
    "\u{28fe}", "\u{28fd}", "\u{28fb}", "\u{287f}", "\u{28bf}", "\u{289f}", "\u{28af}", "\u{28f7}",
];

/// Render the Queue tab.
pub fn render_queue(f: &mut Frame, area: Rect, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),    // downloads list
            Constraint::Length(3), // URL input bar
        ])
        .split(area);

    render_downloads(f, chunks[0], app);
    render_url_bar(f, chunks[1], app);
}

fn render_downloads(f: &mut Frame, area: Rect, app: &App) {
    let active = app
        .slots
        .iter()
        .filter(|s| matches!(s.state, SlotState::Downloading { .. }))
        .count();
    let total = app.slots.len();

    let title = if total == 0 {
        " Downloads ".to_string()
    } else {
        format!(" Downloads  {} active / {} total ", active, total)
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::SURFACE0))
        .style(Style::default().bg(theme::BASE));

    if app.slots.is_empty() {
        let empty = Paragraph::new("\n  No downloads yet.\n  Paste a URL in the bar below and press Enter.")
            .block(block)
            .style(Style::default().fg(theme::SUBTEXT));
        f.render_widget(empty, area);
        return;
    }

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Give each slot at least 3 rows; cap total so we don't go out of bounds.
    let count = app.slots.len() as u16;
    let slot_h = (inner.height / count.max(1)).max(3);
    let constraints: Vec<Constraint> = app.slots.iter().map(|_| Constraint::Min(slot_h)).collect();
    let slot_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    let spinner = SPINNER[app.spinner_frame as usize % SPINNER.len()];

    for (slot, &slot_area) in app.slots.iter().zip(slot_areas.iter()) {
        let title_str = slot.title.as_deref().unwrap_or(slot.url.as_str());
        let display = match &slot.artist {
            Some(a) => format!("{} — {}", a, title_str),
            None => title_str.to_string(),
        };
        let truncated = truncate(&display, slot_area.width.saturating_sub(14) as usize);

        match &slot.state {
            SlotState::Pending => {
                f.render_widget(
                    Paragraph::new(format!("  ⏳ [{}]  {}", slot.format.label(), truncated))
                        .style(Style::default().fg(theme::SUBTEXT)),
                    slot_area,
                );
            }

            SlotState::Fetching => {
                f.render_widget(
                    Paragraph::new(format!("  {} [{}]  {}", spinner, slot.format.label(), truncated))
                        .style(Style::default().fg(theme::YELLOW)),
                    slot_area,
                );
            }

            SlotState::Downloading {
                percent,
                speed_mbs,
                eta_secs,
            } => {
                if slot_area.height < 2 {
                    // Cramped: single-line fallback
                    let label = format!(
                        "  {} [{}]  {}  {:3}%  {}",
                        speed_emoji(*speed_mbs),
                        slot.format.label(),
                        truncated,
                        percent,
                        format_speed(*speed_mbs),
                    );
                    f.render_widget(
                        Paragraph::new(label).style(Style::default().fg(theme::PEACH).add_modifier(Modifier::BOLD)),
                        slot_area,
                    );
                    continue;
                }

                let rows = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Min(0)])
                    .split(slot_area);

                // Row 0: title line
                f.render_widget(
                    Paragraph::new(format!(
                        "  {} [{}]  {}",
                        speed_emoji(*speed_mbs),
                        slot.format.label(),
                        truncated,
                    ))
                    .style(Style::default().fg(theme::PEACH).add_modifier(Modifier::BOLD)),
                    rows[0],
                );

                // Row 1: Gauge
                let gauge_label = format!(" {:3}%  {}  ETA {}s ", percent, format_speed(*speed_mbs), eta_secs);
                f.render_widget(
                    Gauge::default()
                        .gauge_style(Style::default().fg(theme::BLUE).bg(theme::SURFACE0))
                        .percent(*percent as u16)
                        .label(gauge_label),
                    rows[1],
                );
            }

            SlotState::Done { .. } => {
                f.render_widget(
                    Paragraph::new(format!("  ✅ [{}]  {}    [r] Reveal", slot.format.label(), truncated))
                        .style(Style::default().fg(theme::GREEN)),
                    slot_area,
                );
            }

            SlotState::Failed { reason } => {
                let reason_short = truncate(reason, 36);
                f.render_widget(
                    Paragraph::new(format!(
                        "  ✖ [{}]  {}  —  {}",
                        slot.format.label(),
                        truncated,
                        reason_short,
                    ))
                    .style(Style::default().fg(theme::RED)),
                    slot_area,
                );
            }
        }
    }
}

fn render_url_bar(f: &mut Frame, area: Rect, app: &App) {
    // Blinking cursor: on for 30 frames, off for 30 frames (60fps → 1 Hz blink)
    let cursor = if app.spinner_frame % 60 < 30 { "│" } else { " " };

    let cookie_badge = if app.cookies_file.is_some() {
        "  [🍪 cookies set]"
    } else {
        ""
    };
    let prompt = format!(" URL ❯ {}{}{}  [Enter] Preview", app.url_input, cursor, cookie_badge);

    let bar = Paragraph::new(prompt)
        .block(
            Block::default()
                .title(" Add Download ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme::LAVENDER)),
        )
        .style(Style::default().fg(theme::TEXT));

    f.render_widget(bar, area);
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn speed_emoji(speed: f64) -> &'static str {
    if speed < 1.0 {
        "🐌"
    } else if speed < 5.0 {
        "⚡"
    } else if speed < 15.0 {
        "🚀"
    } else {
        "💨"
    }
}

fn format_speed(speed: f64) -> String {
    format!("{:.1} MB/s", speed)
}

/// Truncate to `max_chars` Unicode scalar values, appending `…` if truncated.
fn truncate(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let mut chars = s.chars();
    let out: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{}…", out)
    } else {
        out
    }
}
