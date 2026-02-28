//! History panel: completed downloads table with keyboard scrolling + live search filter.

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table};
use ratatui::Frame;

use crate::app::{App, ClickTarget};

/// Render the History panel (right column of the Downloads tab).
pub fn render_history(f: &mut Frame, area: Rect, app: &mut App) {
    // ── Update Filter and Sort (Centralized) ──────────────────────────────────
    app.update_history_filter();
    let filtered_indices = app.history_filtered_indices.clone();

    // ── Filter bar (shown when search is active or filter is non-empty) ───────
    let mut top_offset: u16 = 0;
    let show_filter_bar = app.history_search_mode || !app.history_filter.is_empty();

    // Build block title
    let sort_label = app.history_sort.label();
    let scroll_hint = if app.history_scroll > 0 {
        format!("  ↑{}", app.history_scroll)
    } else {
        String::new()
    };
    let block_title = if !app.history_filter.is_empty() {
        format!(
            " Download History  {}/{} matches  {} {}",
            filtered_indices.len(),
            app.history.len(),
            sort_label,
            scroll_hint,
        )
    } else {
        format!(
            " Download History  {} entries  {} {}",
            app.history.len(),
            sort_label,
            scroll_hint
        )
    };

    let block = Block::default()
        .title(block_title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(app.theme.surface0))
        .style(Style::default().bg(app.theme.base));

    if app.history.is_empty() {
        let empty = Paragraph::new(
            "\n  No completed downloads yet — finish a download to see it here.\n\n  \
             Downloads appear here automatically when they complete.",
        )
        .block(block)
        .style(Style::default().fg(app.theme.subtext));
        f.render_widget(empty, area);
        return;
    }

    let inner = block.inner(area);
    f.render_widget(block, area);

    // ── Render filter input bar inside inner area ─────────────────────────────
    if show_filter_bar {
        let cursor = if app.blink_on { "│" } else { " " };
        let filter_text = if app.history_search_mode {
            format!(
                " 🔍 Filter: {}{}  [Enter] Lock  [Esc] Clear",
                app.history_filter, cursor
            )
        } else {
            format!(" 🔍 Filter: {}  [Esc] Clear", app.history_filter)
        };
        let filter_style = Style::default().fg(app.theme.yellow).add_modifier(Modifier::BOLD);
        let filter_area = Rect::new(inner.x, inner.y, inner.width, 1);
        f.render_widget(Paragraph::new(filter_text).style(filter_style), filter_area);
        // Separator line
        let sep_area = Rect::new(inner.x, inner.y + 1, inner.width, 1);
        let sep: String = "─".repeat(inner.width as usize);
        f.render_widget(
            Paragraph::new(sep).style(Style::default().fg(app.theme.surface0)),
            sep_area,
        );
        top_offset = 2;
    }

    let table_area = Rect::new(
        inner.x,
        inner.y + top_offset,
        inner.width,
        inner.height.saturating_sub(top_offset),
    );

    if filtered_indices.is_empty() {
        f.render_widget(
            Paragraph::new("\n  No entries match the filter.").style(Style::default().fg(app.theme.subtext)),
            table_area,
        );
        return;
    }

    let header = Row::new([" ", "Title", "Artist", "Fmt", "Size", "Finished"])
        .style(Style::default().fg(app.theme.lavender).add_modifier(Modifier::BOLD))
        .height(1);

    // ── Adaptive column widths ────────────────────────────────────────────────
    // Scale proportionally to terminal width; clamp to reasonable min/max.
    let w = table_area.width;
    let sel_w: u16 = 2; // [x]
    let fmt_w: u16 = 5;
    let size_w: u16 = 9;
    let date_w: u16 = 12;
    let fixed = sel_w + fmt_w + size_w + date_w + 4 * 2; // 4 × column_spacing=2
    let flex = w.saturating_sub(fixed);
    let title_w = ((flex as u32 * 60 / 100) as u16).clamp(18, 50);
    let artist_w = flex.saturating_sub(title_w).clamp(10, 28);

    // Apply scroll offset.
    let skip = app.history_scroll as usize;
    let highlight_idx = app.history_index;

    let rows: Vec<Row> = filtered_indices
        .iter()
        .skip(skip)
        .enumerate()
        .map(|(i, &display_idx)| {
            let abs_idx = skip + i;
            let entry = app.history.iter().rev().nth(display_idx).unwrap();
            let when = entry.finished_at.format("%H:%M %d/%m").to_string();
            let is_highlighted = abs_idx == highlight_idx;
            let is_multi_selected = app.history_selected.contains(&display_idx);

            let row_style = if is_highlighted {
                Style::default().bg(app.theme.surface0).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let sel_icon = if is_multi_selected { "●" } else { "○" };
            let sel_color = if is_multi_selected {
                app.theme.green
            } else {
                app.theme.surface1
            };

            Row::new([
                Cell::from(sel_icon).style(Style::default().fg(sel_color)),
                Cell::from(entry.title.clone()).style(Style::default().fg(if is_highlighted {
                    app.theme.lavender
                } else {
                    app.theme.text
                })),
                Cell::from(entry.artist.clone()).style(Style::default().fg(app.theme.subtext)),
                Cell::from(entry.format.label()).style(Style::default().fg(app.theme.peach)),
                Cell::from(format!("{:.1} MB", entry.size_mb)).style(Style::default().fg(app.theme.blue)),
                Cell::from(when).style(Style::default().fg(app.theme.subtext)),
            ])
            .style(row_style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(sel_w),
            Constraint::Length(title_w),
            Constraint::Length(artist_w),
            Constraint::Length(fmt_w),
            Constraint::Length(size_w),
            Constraint::Length(date_w),
        ],
    )
    .header(header)
    .column_spacing(2)
    .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    f.render_widget(table, table_area);

    // ── Register click areas for visible rows ────────────────────────────────
    // The table header is at table_area.y.
    // The first data row is at table_area.y + 1.
    let data_start_y = table_area.y + 1;
    let max_data_rows = table_area.height.saturating_sub(1); // excluding header

    for i in 0..(max_data_rows as usize) {
        let abs_filter_idx = skip + i;
        if abs_filter_idx >= filtered_indices.len() {
            break;
        }
        app.click_map.push((
            Rect::new(area.x, data_start_y + i as u16, area.width, 1),
            ClickTarget::HistoryOpenPopup(abs_filter_idx),
        ));
    }

    // ── Hint bar at the bottom ────────────────────────────────────────────────
    let hint_y = area.y + area.height.saturating_sub(1);

    // Left hint
    let left_hint = if app.history_search_mode || !app.history_filter.is_empty() {
        " [↑↓] Navigate  [/] Search  [Esc] Clear filter "
    } else {
        " [↑↓] Navigate  [r/Enter] Reveal  [/] Search  [s] Sort "
    };
    f.render_widget(
        Paragraph::new(left_hint).style(Style::default().fg(app.theme.subtext)),
        Rect::new(area.x + 1, hint_y, area.width.saturating_sub(2), 1),
    );

    // Scroll hint in bottom-right corner if there are more entries below
    let visible_rows = table_area.height.saturating_sub(2) as usize;
    let total_filtered = filtered_indices.len();
    if skip + visible_rows < total_filtered {
        let remaining = total_filtered - skip - visible_rows.min(total_filtered - skip);
        let hint = format!(" ↓ {} more ", remaining);
        let hint_len = hint.len() as u16;
        let hint_area = Rect::new(area.x + area.width.saturating_sub(hint_len + 1), hint_y, hint_len, 1);
        f.render_widget(
            Paragraph::new(hint).style(Style::default().fg(app.theme.subtext)),
            hint_area,
        );
    }
}
