//! History panel: completed downloads table with keyboard scrolling + live search filter.

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table};
use ratatui::Frame;

use crate::app::{App, ClickTarget, HistorySortMode};

/// Render the History panel (right column of the Downloads tab).
pub fn render_history(f: &mut Frame, area: Rect, app: &mut App) {
    // ── Filter-active entries ─────────────────────────────────────────────────
    let filter = app.history_filter.to_lowercase();
    let mut filtered_indices: Vec<usize> = if filter.is_empty() {
        // All entries (in newest-first display order: index = rev position)
        (0..app.history.len()).collect()
    } else {
        app.history
            .iter()
            .rev()
            .enumerate()
            .filter_map(|(display_idx, e)| {
                let matches = e.title.to_lowercase().contains(&filter) || e.artist.to_lowercase().contains(&filter);
                if matches {
                    Some(display_idx)
                } else {
                    None
                }
            })
            .collect()
    };

    // ── Sort filtered entries according to history_sort mode ──────────────────
    // filtered_indices are display indices (0 = newest) into app.history.iter().rev()
    match app.history_sort {
        HistorySortMode::DateDesc => {} // already newest-first
        HistorySortMode::DateAsc => filtered_indices.reverse(),
        HistorySortMode::SizeDesc => {
            let history = &app.history;
            filtered_indices.sort_by(|&a, &b| {
                let ea = history.iter().rev().nth(a).map_or(0.0, |e| e.size_mb);
                let eb = history.iter().rev().nth(b).map_or(0.0, |e| e.size_mb);
                eb.partial_cmp(&ea).unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        HistorySortMode::TitleAsc => {
            let history = &app.history;
            filtered_indices.sort_by(|&a, &b| {
                let ta = history.iter().rev().nth(a).map_or("", |e| e.title.as_str());
                let tb = history.iter().rev().nth(b).map_or("", |e| e.title.as_str());
                ta.to_lowercase().cmp(&tb.to_lowercase())
            });
        }
    }

    // ── Filter bar (shown when search is active or filter is non-empty) ───────
    let mut top_offset: u16 = 0;
    let show_filter_bar = app.history_search_mode || !app.history_filter.is_empty();

    // Build block title
    let sort_label = app.history_sort.label();
    let block_title = if !filter.is_empty() {
        format!(
            " Download History  {}/{} matches  {} ",
            filtered_indices.len(),
            app.history.len(),
            sort_label,
        )
    } else {
        format!(" Download History  {} entries  {} ", app.history.len(), sort_label)
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

    let header = Row::new(["Title", "Artist", "Fmt", "Size", "Finished"])
        .style(Style::default().fg(app.theme.lavender).add_modifier(Modifier::BOLD))
        .height(1);

    // ── Adaptive column widths ────────────────────────────────────────────────
    // Scale proportionally to terminal width; clamp to reasonable min/max.
    let w = table_area.width;
    let fmt_w: u16 = 5;
    let size_w: u16 = 9;
    let date_w: u16 = 12;
    let fixed = fmt_w + size_w + date_w + 3 * 2; // 3 × column_spacing=2
    let flex = w.saturating_sub(fixed);
    let title_w = ((flex as u32 * 60 / 100) as u16).clamp(18, 50);
    let artist_w = flex.saturating_sub(title_w).clamp(10, 28);

    // Apply scroll offset (filtered display, newest-first ordering).
    let skip = app.history_scroll as usize;
    let selected_in_view = 0usize; // selected row is always 0 (top of visible)

    let rows: Vec<Row> = filtered_indices
        .iter()
        .skip(skip)
        .enumerate()
        .map(|(i, &display_idx)| {
            // display_idx is the rev-position in app.history
            let entry = app.history.iter().rev().nth(display_idx).unwrap();
            let when = entry.finished_at.format("%H:%M %d/%m").to_string();
            let is_selected = i == selected_in_view;
            let row_style = if is_selected {
                Style::default().bg(app.theme.surface0)
            } else {
                Style::default()
            };
            Row::new([
                Cell::from(entry.title.clone()).style(Style::default().fg(app.theme.text)),
                Cell::from(entry.artist.clone()).style(Style::default().fg(app.theme.subtext)),
                Cell::from(entry.format.label()).style(Style::default().fg(app.theme.peach)),
                Cell::from(format!("{:.1} MB", entry.size_mb)).style(Style::default().fg(app.theme.blue)),
                Cell::from(when).style(Style::default().fg(app.theme.subtext)),
            ])
            .style(row_style)
        })
        .collect();

    // Scroll hint in title if scrolled
    let scroll_hint = if skip > 0 {
        format!(" ↑ {} hidden ", skip)
    } else {
        String::new()
    };
    // Redraw block with scroll hint — note: we already rendered the outer block above,
    // but we need a borderless block here just for the table's block parameter.
    let inner_block = Block::default()
        .title(if skip > 0 { scroll_hint } else { String::new() })
        .borders(Borders::NONE);

    let table = Table::new(
        rows,
        [
            Constraint::Length(title_w),
            Constraint::Length(artist_w),
            Constraint::Length(fmt_w),
            Constraint::Length(size_w),
            Constraint::Length(date_w),
        ],
    )
    .header(header)
    .block(inner_block)
    .column_spacing(2)
    .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    f.render_widget(table, table_area);

    // ── Register click areas for each visible data row ────────────────────────
    // Layout: table_area.y=header row, table_area.y+1..=data rows (1 row each)
    let data_start_y = table_area.y + 1; // after header row
    let max_data_rows = table_area.height.saturating_sub(2) as usize; // header + hint
    for i in 0..max_data_rows {
        let abs_filter_idx = skip + i;
        if abs_filter_idx >= filtered_indices.len() {
            break;
        }
        let display_idx = filtered_indices[abs_filter_idx];
        app.click_map.push((
            Rect::new(area.x + 1, data_start_y + i as u16, area.width.saturating_sub(2), 1),
            ClickTarget::HistoryOpenPopup(display_idx),
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
