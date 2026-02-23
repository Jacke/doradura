//! Lyrics tab: search input and scrollable lyrics display.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;

const SPINNER: &[&str] = &[
    "\u{28fe}", "\u{28fd}", "\u{28fb}", "\u{287f}", "\u{28bf}", "\u{289f}", "\u{28af}", "\u{28f7}",
];

/// Render the Lyrics tab.
pub fn render_lyrics(f: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(area);

    render_search_bar(f, chunks[0], app);
    render_lyrics_content(f, chunks[1], app);
}

fn render_search_bar(f: &mut Frame, area: Rect, app: &App) {
    // Blinking cursor
    let cursor = if app.blink_on { "│" } else { " " };

    let prompt = format!(" 🎵 Search ❯ {}{}   [Enter to search]", app.lyrics_query, cursor);

    let bar = Paragraph::new(prompt)
        .block(
            Block::default()
                .title(" Lyrics Search ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(app.theme.lavender)),
        )
        .style(Style::default().fg(app.theme.text));

    f.render_widget(bar, area);
}

fn render_lyrics_content(f: &mut Frame, area: Rect, app: &App) {
    let content_title = match &app.lyrics_result {
        Some(r) => format!(" {} — {} ", r.artist, r.title),
        None => " Lyrics ".to_string(),
    };

    let block = Block::default()
        .title(content_title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(app.theme.surface0))
        .style(Style::default().bg(app.theme.base));

    if app.lyrics_loading {
        let spinner = SPINNER[app.spinner_frame as usize % SPINNER.len()];
        let loading = Paragraph::new(format!("\n  {} Searching for lyrics…", spinner))
            .block(block)
            .style(Style::default().fg(app.theme.yellow));
        f.render_widget(loading, area);
        return;
    }

    match &app.lyrics_result {
        Some(result) => {
            let scroll_info = if app.lyrics_scroll > 0 {
                format!("  (↑{})", app.lyrics_scroll)
            } else {
                String::new()
            };

            let block = Block::default()
                .title(format!(" {} — {}{}", result.artist, result.title, scroll_info))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(app.theme.surface0))
                .style(Style::default().bg(app.theme.base));

            let p = Paragraph::new(result.lyrics.as_str())
                .block(block)
                .style(Style::default().fg(app.theme.text))
                .wrap(Wrap { trim: false })
                .scroll((app.lyrics_scroll, 0));
            f.render_widget(p, area);
        }

        None => {
            let text: Vec<Line> = if app.lyrics_query.is_empty() {
                vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        "  Type an artist + song title above, then press Enter.",
                        Style::default().fg(app.theme.subtext),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        "  Powered by LRCLIB",
                        Style::default().fg(app.theme.subtext).add_modifier(Modifier::ITALIC),
                    )),
                ]
            } else {
                vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        "  No results found — try a different search.",
                        Style::default().fg(app.theme.red),
                    )),
                ]
            };

            let p = Paragraph::new(text).block(block);
            f.render_widget(p, area);
        }
    }
}
