//! History tab: completed downloads table with keyboard scrolling.

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table};
use ratatui::Frame;

use crate::app::App;
use crate::theme;

/// Render the History tab.
pub fn render_history(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .title(format!(" Download History  {} entries ", app.history.len()))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::SURFACE0))
        .style(Style::default().bg(theme::BASE));

    if app.history.is_empty() {
        let empty = Paragraph::new(
            "\n  No completed downloads yet — finish a download to see it here.\n\n  \
             Downloads appear here automatically when they complete.",
        )
        .block(block)
        .style(Style::default().fg(theme::SUBTEXT));
        f.render_widget(empty, area);
        return;
    }

    let header = Row::new(["Title", "Artist", "Fmt", "Size", "Finished"])
        .style(Style::default().fg(theme::LAVENDER).add_modifier(Modifier::BOLD))
        .height(1);

    // Apply scroll offset (newest-first ordering).
    let skip = app.history_scroll as usize;
    let selected_in_view = 0usize; // selected row is always 0 (top of visible, = history_scroll)
    let rows: Vec<Row> = app
        .history
        .iter()
        .rev()
        .skip(skip)
        .enumerate()
        .map(|(i, entry)| {
            let when = entry.finished_at.format("%H:%M %d/%m").to_string();
            let is_selected = i == selected_in_view;
            let row_style = if is_selected {
                Style::default().bg(theme::SURFACE0)
            } else {
                Style::default()
            };
            Row::new([
                Cell::from(entry.title.clone()).style(Style::default().fg(theme::TEXT)),
                Cell::from(entry.artist.clone()).style(Style::default().fg(theme::SUBTEXT)),
                Cell::from(entry.format.label()).style(Style::default().fg(theme::PEACH)),
                Cell::from(format!("{:.1} MB", entry.size_mb)).style(Style::default().fg(theme::BLUE)),
                Cell::from(when).style(Style::default().fg(theme::SUBTEXT)),
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
    let block = Block::default()
        .title(format!(
            " Download History  {} entries{}",
            app.history.len(),
            scroll_hint,
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::SURFACE0))
        .style(Style::default().bg(theme::BASE));

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(35),
            Constraint::Percentage(25),
            Constraint::Length(5),
            Constraint::Length(9),
            Constraint::Length(12),
        ],
    )
    .header(header)
    .block(block)
    .column_spacing(2)
    .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    f.render_widget(table, area);

    // Key hint in bottom-left corner
    let hint_area = ratatui::layout::Rect::new(area.x + 1, area.y + area.height.saturating_sub(1), 30, 1);
    f.render_widget(
        Paragraph::new(" [↑↓] Navigate  [r/Enter] Reveal ").style(Style::default().fg(theme::SUBTEXT)),
        hint_area,
    );

    // Scroll hint in bottom-right corner if there are more entries below
    let visible_rows = area.height.saturating_sub(4) as usize; // borders + header
    let total = app.history.len();
    if skip + visible_rows < total {
        let hint = format!(" ↓ {} more ", total - skip - visible_rows.min(total - skip));
        let hint_len = hint.len() as u16;
        let hint_area = ratatui::layout::Rect::new(
            area.x + area.width.saturating_sub(hint_len + 1),
            area.y + area.height.saturating_sub(1),
            hint_len,
            1,
        );
        f.render_widget(
            Paragraph::new(hint).style(Style::default().fg(theme::SUBTEXT)),
            hint_area,
        );
    }
}
