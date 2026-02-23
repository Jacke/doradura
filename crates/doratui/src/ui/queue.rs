//! Queue tab: active downloads with waveform gauges, sparklines, and URL input bar.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{App, SlotState};

const SPINNER: &[&str] = &[
    "\u{28fe}", "\u{28fd}", "\u{28fb}", "\u{287f}", "\u{28bf}", "\u{289f}", "\u{28af}", "\u{28f7}",
];

/// Sparkline characters for speed history (8 levels, lowest to highest).
const SPARK: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Animated waveform characters — one full cycle (14 frames).
const WAVE: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█', '▇', '▆', '▅', '▄', '▃', '▂'];

/// Render the Queue tab (downloads list + stats footer + URL input bar).
pub fn render_queue(f: &mut Frame, area: Rect, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),    // downloads list
            Constraint::Length(1), // stats footer
            Constraint::Length(3), // URL input bar
        ])
        .split(area);

    render_downloads(f, chunks[0], app);
    render_stats_footer(f, chunks[1], app);
    render_url_bar(f, chunks[2], app);
}

fn render_downloads(f: &mut Frame, area: Rect, app: &mut App) {
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
        .border_style(Style::default().fg(app.theme.surface0))
        .style(Style::default().bg(app.theme.base));

    if app.slots.is_empty() {
        let empty = Paragraph::new("\n  No downloads yet.\n  Paste a URL in the bar below and press Enter.")
            .block(block)
            .style(Style::default().fg(app.theme.subtext));
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
    let frame = app.logo_frame as usize;

    // Capture theme values before the slot loop (needed to avoid borrow issues
    // while also mutating slot_screen_rects).
    let theme = app.theme;

    // Clear slot rects; they'll be repopulated during this render.
    app.slot_screen_rects.clear();

    for (i, slot) in app.slots.iter().enumerate() {
        let slot_area = slot_areas[i];

        // Record screen rect for supernova particle spawning.
        app.slot_screen_rects.insert(slot.id, slot_area);

        let title_str = slot.title.as_deref().unwrap_or(slot.url.as_str());
        let display = match &slot.artist {
            Some(a) => format!("{} — {}", a, title_str),
            None => title_str.to_string(),
        };
        let truncated = truncate(&display, slot_area.width.saturating_sub(14) as usize);

        let accent = source_color(&slot.url, &theme);

        match &slot.state {
            SlotState::Pending => {
                f.render_widget(
                    Paragraph::new(format!("  ⏳ [{}]  {}", slot.format.label(), truncated))
                        .style(Style::default().fg(accent)),
                    slot_area,
                );
            }

            SlotState::Fetching => {
                f.render_widget(
                    Paragraph::new(format!("  {} [{}]  {}", spinner, slot.format.label(), truncated))
                        .style(Style::default().fg(accent).add_modifier(Modifier::BOLD)),
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
                        Paragraph::new(label).style(Style::default().fg(accent).add_modifier(Modifier::BOLD)),
                        slot_area,
                    );
                    continue;
                }

                let has_sparkline = !slot.speed_history.is_empty() && slot_area.height >= 3;
                let row_constraints = if has_sparkline {
                    vec![
                        Constraint::Length(1), // title
                        Constraint::Length(1), // waveform gauge
                        Constraint::Length(1), // sparkline
                        Constraint::Min(0),    // padding
                    ]
                } else {
                    vec![Constraint::Length(1), Constraint::Length(1), Constraint::Min(0)]
                };

                let rows = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints(row_constraints)
                    .split(slot_area);

                // Row 0: title line (accent = source domain color)
                f.render_widget(
                    Paragraph::new(format!(
                        "  {} [{}]  {}",
                        speed_emoji(*speed_mbs),
                        slot.format.label(),
                        truncated,
                    ))
                    .style(Style::default().fg(accent).add_modifier(Modifier::BOLD)),
                    rows[0],
                );

                // Row 1: Animated waveform bar with absolute ETA
                render_waveform(f, rows[1], *percent, *speed_mbs, *eta_secs, frame, &theme);

                // Row 2: Speed sparkline (if we have history and space)
                if has_sparkline {
                    let spark_str = build_sparkline(&slot.speed_history);
                    let max_speed = slot.speed_history.iter().cloned().fold(0.0_f64, f64::max);
                    let spark_label = format!("  ⚡ {}", spark_str);
                    let speed_hint = format!("  peak {}", format_speed(max_speed));
                    let spark_line = Line::from(vec![
                        Span::styled(spark_label, Style::default().fg(theme.teal)),
                        Span::styled(speed_hint, Style::default().fg(theme.subtext)),
                    ]);
                    f.render_widget(Paragraph::new(spark_line), rows[2]);
                }
            }

            SlotState::Celebrating { path, started } => {
                // 1-second colour sweep: LAVENDER → GREEN
                let progress = started.elapsed().as_secs_f32().clamp(0.0, 1.0);
                let color = if progress < 0.5 {
                    lerp_color(theme.lavender, theme.green, progress * 2.0)
                } else {
                    theme.green
                };
                let path_display = truncate(path, slot_area.width.saturating_sub(18) as usize);
                f.render_widget(
                    Paragraph::new(format!("  ✨ [{}]  {}    saved!", slot.format.label(), path_display,))
                        .style(Style::default().fg(color).add_modifier(Modifier::BOLD)),
                    slot_area,
                );
            }

            SlotState::Done { path } => {
                let path_display = truncate(path, slot_area.width.saturating_sub(18) as usize);
                f.render_widget(
                    Paragraph::new(format!(
                        "  ✅ [{}]  {}    [r] Reveal",
                        slot.format.label(),
                        path_display,
                    ))
                    .style(Style::default().fg(theme.green)),
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
                    .style(Style::default().fg(theme.red)),
                    slot_area,
                );
            }
        }
    }
}

/// Animated waveform bar: replaces the plain Gauge.
///
/// The waveform chars scroll left based on `frame`.  Columns within the
/// filled portion (≤ percent%) are rendered in a speed-tinted colour;
/// the unfilled portion uses `surface0`.  A text label is overlaid on top.
#[allow(clippy::too_many_arguments)]
fn render_waveform(
    f: &mut Frame,
    area: Rect,
    percent: u8,
    speed_mbs: f64,
    eta_secs: u64,
    frame: usize,
    theme: &crate::theme::ThemeColors,
) {
    let width = area.width as usize;
    if width == 0 {
        return;
    }
    let filled_cols = (width * percent as usize / 100).min(width);
    let bar_color = speed_bar_color(speed_mbs, theme);

    // Build animated waveform spans.
    let spans: Vec<Span> = (0..width)
        .map(|col| {
            let wave_idx = (col * 3 + frame * 2) % WAVE.len();
            let ch = WAVE[wave_idx].to_string();
            if col < filled_cols {
                Span::styled(ch, Style::default().fg(bar_color))
            } else {
                Span::styled(ch, Style::default().fg(theme.surface0))
            }
        })
        .collect();
    f.render_widget(Paragraph::new(Line::from(spans)), area);

    // Overlay text label: "  42%  8.3 MB/s  ~14:05 "
    let eta_time = chrono::Local::now() + chrono::Duration::seconds(eta_secs as i64);
    let eta_str = eta_time.format("%H:%M").to_string();
    let label = format!(" {:3}%  {}  ~{} ", percent, format_speed(speed_mbs), eta_str);
    let label_len = label.chars().count() as u16;
    if label_len < area.width {
        let label_x = area.x + (area.width - label_len) / 2;
        f.render_widget(
            Paragraph::new(label).style(Style::default().fg(theme.text).add_modifier(Modifier::BOLD)),
            Rect::new(label_x, area.y, label_len, 1),
        );
    }
}

/// Render the 1-line stats footer at the bottom of the downloads panel.
fn render_stats_footer(f: &mut Frame, area: Rect, app: &App) {
    // Count active downloads and aggregate speed.
    let active_count = app
        .slots
        .iter()
        .filter(|s| matches!(s.state, SlotState::Downloading { .. }))
        .count();

    let total_speed: f64 = app
        .slots
        .iter()
        .filter_map(|s| {
            if let SlotState::Downloading { speed_mbs, .. } = s.state {
                Some(speed_mbs)
            } else {
                None
            }
        })
        .sum();

    // Count today's completed downloads from history.
    let today = chrono::Local::now().date_naive();
    let done_today = app
        .history
        .iter()
        .filter(|e| e.finished_at.date_naive() == today)
        .count();

    let total_mb_today: f64 = app
        .history
        .iter()
        .filter(|e| e.finished_at.date_naive() == today)
        .map(|e| e.size_mb)
        .sum();

    // Uptime
    let uptime_secs = app.session_start.elapsed().as_secs();
    let uptime_str = format!(
        "{:02}:{:02}:{:02}",
        uptime_secs / 3600,
        (uptime_secs % 3600) / 60,
        uptime_secs % 60
    );

    let mut spans: Vec<Span> = Vec::new();
    let dim = |s: String| Span::styled(s, Style::default().fg(app.theme.subtext));
    let hi = |s: String| Span::styled(s, Style::default().fg(app.theme.lavender));

    if active_count > 0 {
        spans.push(Span::styled(" ↓ ", Style::default().fg(app.theme.peach)));
        spans.push(hi(format!("{} active", active_count)));
        spans.push(dim("   ".to_string()));
        if total_speed > 0.0 {
            spans.push(Span::styled("⚡ ", Style::default().fg(app.theme.teal)));
            spans.push(hi(format_speed(total_speed).to_string()));
            spans.push(dim("   ".to_string()));
        }
    } else {
        spans.push(dim(" ".to_string()));
    }

    if done_today > 0 {
        spans.push(Span::styled("✓ ", Style::default().fg(app.theme.green)));
        spans.push(hi(format!("{} done today", done_today)));
        if total_mb_today > 0.0 {
            spans.push(dim(format!("  ({:.0} MB)", total_mb_today)));
        }
        spans.push(dim("   ".to_string()));
    }

    spans.push(Span::styled("⏱ ", Style::default().fg(app.theme.subtext)));
    spans.push(dim(uptime_str));

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_url_bar(f: &mut Frame, area: Rect, app: &App) {
    let cursor = if app.blink_on { "│" } else { " " };

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
                .border_style(Style::default().fg(app.theme.lavender)),
        )
        .style(Style::default().fg(app.theme.text));

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

/// Pick an accent colour for a slot based on its source domain.
///
/// This gives each source a distinctive ambient tint so glancing at the queue
/// makes it immediately obvious which service each slot came from.
fn source_color(url: &str, theme: &crate::theme::ThemeColors) -> Color {
    if url.contains("youtube.com") || url.contains("youtu.be") || url.contains("music.youtube.com") {
        theme.red
    } else if url.contains("soundcloud.com") {
        theme.peach
    } else if url.contains("instagram.com") {
        theme.mauve
    } else if url.contains("twitter.com") || url.contains("x.com") {
        theme.blue
    } else if url.contains("tiktok.com") {
        theme.green
    } else if url.contains("vimeo.com") {
        theme.teal
    } else {
        theme.lavender // generic / direct HTTP
    }
}

/// Pick waveform fill color based on download speed.
fn speed_bar_color(speed_mbs: f64, theme: &crate::theme::ThemeColors) -> Color {
    if speed_mbs >= 10.0 {
        theme.green
    } else if speed_mbs >= 3.0 {
        theme.teal
    } else {
        theme.blue
    }
}

/// Build a Unicode sparkline string from the last N speed samples.
fn build_sparkline(history: &std::collections::VecDeque<f64>) -> String {
    if history.is_empty() {
        return String::new();
    }
    let max = history.iter().cloned().fold(0.0_f64, f64::max).max(0.001);
    history
        .iter()
        .map(|&v| {
            let idx = ((v / max) * (SPARK.len() - 1) as f64) as usize;
            SPARK[idx.min(SPARK.len() - 1)]
        })
        .collect()
}

/// Linearly interpolate between two ratatui colours (for the celebration sweep).
fn lerp_color(from: Color, to: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    fn rgb(c: Color) -> (u8, u8, u8) {
        match c {
            Color::Rgb(r, g, b) => (r, g, b),
            _ => (255, 255, 255),
        }
    }
    let (fr, fg, fb) = rgb(from);
    let (tr, tg, tb) = rgb(to);
    let r = (fr as f32 * (1.0 - t) + tr as f32 * t) as u8;
    let g = (fg as f32 * (1.0 - t) + tg as f32 * t) as u8;
    let b = (fb as f32 * (1.0 - t) + tb as f32 * t) as u8;
    Color::Rgb(r, g, b)
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
