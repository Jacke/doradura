//! Settings tab renderer for the dora TUI.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{App, ClickTarget};
use crate::settings::{AUDIO_BITRATES, FORMATS, RATE_LIMITS, THEME_FLAVOURS, VIDEO_QUALITIES};
use crate::theme::{palette, CatppuccinFlavour, ThemeColors};

// ── Settings item descriptors ─────────────────────────────────────────────────

/// What kind of interaction a settings item supports.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    /// Cycle through a fixed list of strings.
    Cycle,
    /// Free-form text / path entry.
    Text,
}

/// A single settings row (section header or item).
pub struct SettingsItem {
    pub label: &'static str,
    pub kind: ItemKind,
    pub choices: &'static [&'static str],
}

/// All 12 editable items (no section headers — those are rendered separately).
pub const ITEMS: &[SettingsItem] = &[
    // ── yt-dlp (indices 0-5) ──────────────────────────────────────────────
    SettingsItem {
        label: "Binary path",
        kind: ItemKind::Text,
        choices: &[],
    },
    SettingsItem {
        label: "Output folder",
        kind: ItemKind::Text,
        choices: &[],
    },
    SettingsItem {
        label: "Audio bitrate",
        kind: ItemKind::Cycle,
        choices: AUDIO_BITRATES,
    },
    SettingsItem {
        label: "Video quality",
        kind: ItemKind::Cycle,
        choices: VIDEO_QUALITIES,
    },
    SettingsItem {
        label: "Rate limit",
        kind: ItemKind::Cycle,
        choices: RATE_LIMITS,
    },
    SettingsItem {
        label: "Cookies file",
        kind: ItemKind::Text,
        choices: &[],
    },
    // ── Instagram (indices 6-7) ───────────────────────────────────────────
    SettingsItem {
        label: "Cookies file",
        kind: ItemKind::Text,
        choices: &[],
    },
    SettingsItem {
        label: "Doc ID",
        kind: ItemKind::Text,
        choices: &[],
    },
    // ── Conversion (indices 8-10) ─────────────────────────────────────────
    SettingsItem {
        label: "Default format",
        kind: ItemKind::Cycle,
        choices: FORMATS,
    },
    SettingsItem {
        label: "MP3 bitrate",
        kind: ItemKind::Cycle,
        choices: AUDIO_BITRATES,
    },
    SettingsItem {
        label: "MP4 quality",
        kind: ItemKind::Cycle,
        choices: VIDEO_QUALITIES,
    },
    // ── Appearance (index 11) ─────────────────────────────────────────────
    SettingsItem {
        label: "Theme",
        kind: ItemKind::Cycle,
        choices: THEME_FLAVOURS,
    },
];

// ── Section layout ────────────────────────────────────────────────────────────

/// (section_label, first_item_index, item_count)
const SECTIONS: &[(&str, usize, usize)] = &[
    ("yt-dlp", 0, 6),
    ("Instagram", 6, 2),
    ("Conversion", 8, 3),
    ("Appearance", 11, 1),
];

// ── Public renderer ───────────────────────────────────────────────────────────

pub fn render_settings(f: &mut Frame, area: Rect, app: &mut App) {
    let block = Block::default()
        .title(" ⚙  Settings ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(app.theme.surface0))
        .style(Style::default().bg(app.theme.base));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // ── Inner layout: content + hint bar ─────────────────────────────────────
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    render_items(f, rows[0], app);
    render_hint_bar(f, rows[1], &app.theme);
}

// ── Items list ────────────────────────────────────────────────────────────────

fn render_items(f: &mut Frame, area: Rect, app: &mut App) {
    let cursor = app.settings_cursor;
    let editing = app.settings_editing;
    let blink = if app.blink_on { "│" } else { " " };

    let mut lines: Vec<Line> = vec![Line::from("")];
    // row_y tracks actual terminal y for each rendered line (for click map)
    // starts at area.y + 1 (blank line at top)
    let mut row_y = area.y + 1;

    // prefix widths: "    " = 4 chars (arrow column)
    const ARROW_W: u16 = 4;
    const LABEL_W: u16 = 20;

    for &(section_label, first, count) in SECTIONS {
        // Section header line
        lines.push(Line::from(Span::styled(
            format!("  {}", section_label),
            Style::default().fg(app.theme.lavender).add_modifier(Modifier::BOLD),
        )));
        row_y += 1;

        #[allow(clippy::needless_range_loop)]
        for idx in first..first + count {
            let item = &ITEMS[idx];
            let is_selected = idx == cursor;

            // Current value string
            let value_str = get_value(app, idx);
            let value_char_len = value_str.chars().count();

            // Format the value field
            let value_display = if editing && is_selected && item.kind == ItemKind::Text {
                format!("{}{}  ", app.settings_edit_buf, blink)
            } else if item.kind == ItemKind::Cycle {
                format!("← {} →", value_str)
            } else {
                let display = if value_str.is_empty() {
                    "(none)".to_string()
                } else {
                    value_str
                };
                format!("  {}  ", display)
            };

            // Selector arrow
            let arrow = if is_selected { "  ▶ " } else { "    " };
            let arrow_style = if is_selected {
                Style::default().fg(app.theme.peach).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(app.theme.subtext)
            };

            // Label style
            let label_style = if is_selected {
                Style::default().fg(app.theme.text).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(app.theme.text)
            };

            // Value style
            let value_style = if is_selected && editing {
                Style::default().fg(app.theme.yellow)
            } else if is_selected {
                Style::default().fg(app.theme.lavender).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(app.theme.subtext)
            };

            // Pad label to fixed width
            let label_padded = format!("{:<20}", item.label);

            let line = Line::from(vec![
                Span::styled(arrow, arrow_style),
                Span::styled(label_padded, label_style),
                Span::styled(value_display, value_style),
            ]);
            lines.push(line);

            // ── Register click areas for this row ────────────────────────────
            // Clicking anywhere on the row selects it
            app.click_map.push((
                Rect::new(area.x, row_y, area.width, 1),
                ClickTarget::SettingsSelectItem(idx),
            ));

            // For Cycle items: register ← and → separately.
            // value_display = "← VALUE →", rendered immediately after label.
            // Layout: [ARROW_W=4][LABEL_W=20][← VALUE →]
            if item.kind == ItemKind::Cycle {
                let cycle_x = area.x + ARROW_W + LABEL_W; // no leading spaces for cycle items
                                                          // "← " — click zone width 2 (arrow + space)
                app.click_map
                    .push((Rect::new(cycle_x, row_y, 2, 1), ClickTarget::SettingsCycleLeft(idx)));
                // " →" — skip "← " (2) + value chars + " " (1) = at col 3+val_len
                let val_len = value_char_len as u16;
                let arrow_r_x = cycle_x + 2 + val_len + 1; // "← " + value + " "
                app.click_map
                    .push((Rect::new(arrow_r_x, row_y, 2, 1), ClickTarget::SettingsCycleRight(idx)));
            }

            row_y += 1;
        }

        // blank line between sections
        lines.push(Line::from(""));
        row_y += 1;
    }

    f.render_widget(Paragraph::new(lines), area);
}

fn render_hint_bar(f: &mut Frame, area: Rect, theme: &ThemeColors) {
    let k = |s: &'static str| Span::styled(s, Style::default().fg(theme.peach).add_modifier(Modifier::BOLD));
    let d = |s: &'static str| Span::styled(s, Style::default().fg(theme.subtext));
    let sep = || Span::styled("  ", Style::default());

    let hints = Line::from(vec![
        k(" [↑↓]"),
        d(" Navigate"),
        sep(),
        k("[←→]"),
        d(" Cycle"),
        sep(),
        k("[Enter]"),
        d(" Edit"),
        sep(),
        k("[o]"),
        d(" Browse"),
        sep(),
        k("[s]"),
        d(" Save"),
        sep(),
        k("[r]"),
        d(" Reset"),
        sep(),
        k("[Esc]"),
        d(" Cancel"),
    ]);
    f.render_widget(Paragraph::new(hints), area);
}

// ── Value accessors ───────────────────────────────────────────────────────────

/// Get the current string value for settings item `idx` from app state.
pub fn get_value(app: &App, idx: usize) -> String {
    let s = &app.settings;
    match idx {
        0 => s.ytdlp_bin.clone(),
        1 => s.output_folder.clone(),
        2 => s.audio_bitrate.clone(),
        3 => s.video_quality.clone(),
        4 => s.rate_limit.clone(),
        5 => s.ytdlp_cookies.clone(),
        6 => s.instagram_cookies.clone(),
        7 => s.instagram_doc_id.clone(),
        8 => s.default_format.clone(),
        9 => s.default_mp3_bitrate.clone(),
        10 => s.default_mp4_quality.clone(),
        11 => s.theme_flavour.label().to_string(),
        _ => String::new(),
    }
}

/// Set a string value for settings item `idx` into app.settings.
/// For idx 11 (theme), also updates `app.theme` immediately.
pub fn set_value(app: &mut App, idx: usize, value: String) {
    match idx {
        0 => app.settings.ytdlp_bin = value,
        1 => app.settings.output_folder = value,
        2 => app.settings.audio_bitrate = value,
        3 => app.settings.video_quality = value,
        4 => app.settings.rate_limit = value,
        5 => app.settings.ytdlp_cookies = value,
        6 => app.settings.instagram_cookies = value,
        7 => app.settings.instagram_doc_id = value,
        8 => app.settings.default_format = value,
        9 => app.settings.default_mp3_bitrate = value,
        10 => app.settings.default_mp4_quality = value,
        11 => {
            let flavour = CatppuccinFlavour::from_label(&value);
            app.settings.theme_flavour = flavour;
            app.theme = palette(flavour);
        }
        _ => {}
    }
}

/// Cycle a Cycle-type item forward (+1) or backward (-1).
pub fn cycle_value(app: &mut App, idx: usize, delta: i32) {
    let item = &ITEMS[idx];
    if item.kind != ItemKind::Cycle || item.choices.is_empty() {
        return;
    }
    let current = get_value(app, idx);
    let pos = item.choices.iter().position(|&c| c == current).unwrap_or(0);
    let len = item.choices.len();
    let next = if delta >= 0 {
        (pos + 1) % len
    } else {
        (pos + len - 1) % len
    };
    set_value(app, idx, item.choices[next].to_string());
}
