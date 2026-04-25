//! Lyrics tab: search input and scrollable lyrics display.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use crate::app::{App, ClickTarget};

const SPINNER: &[&str] = &[
    "\u{28fe}", "\u{28fd}", "\u{28fb}", "\u{287f}", "\u{28bf}", "\u{289f}", "\u{28af}", "\u{28f7}",
];

/// Render the Lyrics tab.
pub fn render_lyrics(f: &mut Frame, area: Rect, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(area);

    render_search_bar(f, chunks[0], app);

    match app.lyrics_view_mode {
        crate::app::LyricsViewMode::Lyrics => render_lyrics_content(f, chunks[1], app),
        crate::app::LyricsViewMode::ArtistSongs => render_artist_songs(f, chunks[1], app),
    }
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

fn render_lyrics_content(f: &mut Frame, area: Rect, app: &mut App) {
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

            let outer_block = Block::default()
                .title(format!(" Lyrics Viewer{} ", scroll_info))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(app.theme.surface0))
                .style(Style::default().bg(app.theme.base));

            let inner = outer_block.inner(area);
            f.render_widget(outer_block, area);

            // Split inner area: Header (fixed) and Lyrics (scrollable)
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(8), // metadata + divider
                    Constraint::Min(0),    // lyrics
                ])
                .split(inner);

            // 1. Render Fixed Header
            let mut header_lines = Vec::new();
            header_lines.push(Line::from(""));

            let artist_span = Span::styled(
                &result.artist,
                Style::default()
                    .fg(app.theme.lavender)
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED),
            );
            header_lines.push(Line::from(vec![
                Span::styled("  Artist: ", Style::default().fg(app.theme.subtext)),
                artist_span,
            ]));

            // Register click for artist (Aggressive area)
            app.click_map.push((
                Rect::new(chunks[0].x, chunks[0].y, chunks[0].width, 3), // Cover top part of header
                ClickTarget::ArtistClick(result.artist_id, result.artist.clone()),
            ));

            header_lines.push(Line::from(vec![
                Span::styled("  Title:  ", Style::default().fg(app.theme.subtext)),
                Span::styled(
                    &result.title,
                    Style::default().fg(app.theme.text).add_modifier(Modifier::BOLD),
                ),
            ]));

            if let Some(album) = &result.album {
                header_lines.push(Line::from(vec![
                    Span::styled("  Album:  ", Style::default().fg(app.theme.subtext)),
                    Span::styled(album, Style::default().fg(app.theme.peach)),
                ]));
            }

            if let Some(date) = &result.release_date {
                header_lines.push(Line::from(vec![
                    Span::styled("  Date:   ", Style::default().fg(app.theme.subtext)),
                    Span::styled(date, Style::default().fg(app.theme.subtext)),
                ]));
            }

            header_lines.push(Line::from(""));
            header_lines.push(Line::from(Span::styled(
                format!("  {}", "─".repeat(chunks[0].width.saturating_sub(6) as usize)),
                Style::default().fg(app.theme.surface0),
            )));

            f.render_widget(Paragraph::new(header_lines), chunks[0]);

            // 2. Render Scrollable Lyrics
            let mut lyrics_lines = Vec::new();
            for line in result.lyrics.lines() {
                lyrics_lines.push(Line::from(format!("  {}", line)));
            }

            let p = Paragraph::new(lyrics_lines)
                .style(Style::default().fg(app.theme.text))
                .wrap(Wrap { trim: false })
                .scroll((app.lyrics_scroll, 0));
            f.render_widget(p, chunks[1]);
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

fn render_artist_songs(f: &mut Frame, area: Rect, app: &mut App) {
    let artist_name = app
        .lyrics_result
        .as_ref()
        .map(|r| r.artist.as_str())
        .unwrap_or("Artist");
    let outer_block = Block::default()
        .title(format!(" Top songs by {} ", artist_name))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(app.theme.lavender))
        .style(Style::default().bg(app.theme.base));

    let inner = outer_block.inner(area);
    f.render_widget(outer_block, area);

    if app.lyrics_loading {
        let spinner = SPINNER[app.spinner_frame as usize % SPINNER.len()];
        f.render_widget(
            Paragraph::new(format!("\n  {} Fetching discography…", spinner))
                .style(Style::default().fg(app.theme.yellow)),
            inner,
        );
        return;
    }

    if app.artist_songs.is_empty() {
        f.render_widget(
            Paragraph::new("\n  No songs found. Check your Genius token or try another artist.")
                .style(Style::default().fg(app.theme.red)),
            inner,
        );
        return;
    }

    // Dynamic Grid
    let card_w = 30_u16;
    let card_h = 4_u16;
    let cols = (inner.width / card_w).max(1);

    // Auto-scroll logic: calculate skip_rows based on cursor position
    let row_at_cursor = app.artist_songs_cursor as u16 / cols;
    let visible_rows = inner.height.saturating_sub(1) / card_h;

    if row_at_cursor < app.lyrics_scroll {
        app.lyrics_scroll = row_at_cursor;
    } else if visible_rows > 0 && row_at_cursor >= app.lyrics_scroll + visible_rows {
        app.lyrics_scroll = row_at_cursor + 1 - visible_rows;
    }
    let skip_rows = app.lyrics_scroll;

    for (i, song) in app.artist_songs.iter().enumerate() {
        let row_idx = i as u16 / cols;
        let col_idx = i as u16 % cols;

        if row_idx < skip_rows {
            continue;
        }
        let rel_row = row_idx - skip_rows;

        let card_area = Rect::new(
            inner.x + col_idx * card_w,
            inner.y + rel_row * card_h,
            card_w.min(inner.width.saturating_sub(col_idx * card_w)),
            card_h,
        );

        if card_area.y + card_area.height > inner.y + inner.height.saturating_sub(1) {
            continue;
        }

        let is_selected = i == app.artist_songs_cursor;
        let (border_color, border_type) = if is_selected {
            (app.theme.peach, BorderType::Thick)
        } else {
            (app.theme.surface0, BorderType::Rounded)
        };

        let card_block = Block::default()
            .borders(Borders::ALL)
            .border_type(border_type)
            .border_style(Style::default().fg(border_color));

        let song_title = truncate(&song.title, 28);

        let card_content = vec![
            Line::from(""),
            Line::from(vec![
                Span::styled(
                    if is_selected { " ▶ " } else { "   " },
                    Style::default().fg(app.theme.peach),
                ),
                Span::styled(
                    song_title,
                    Style::default()
                        .fg(if is_selected { app.theme.text } else { app.theme.subtext })
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
        ];

        f.render_widget(Paragraph::new(card_content).block(card_block), card_area);

        app.click_map.push((
            card_area,
            ClickTarget::ArtistSongClick(song.artist.clone(), song.title.clone()),
        ));
    }

    // Feature: Load More Card
    let total_songs = app.artist_songs.len();
    let row_idx = total_songs as u16 / cols;
    let col_idx = total_songs as u16 % cols;
    let rel_row = row_idx.saturating_sub(skip_rows);
    let card_area = Rect::new(inner.x + col_idx * card_w, inner.y + rel_row * card_h, card_w, card_h);

    if card_area.y + card_area.height <= inner.y + inner.height.saturating_sub(1) {
        let is_selected = app.artist_songs_cursor == total_songs;
        let border_color = if is_selected {
            app.theme.peach
        } else {
            app.theme.surface0
        };
        let card_block = Block::default()
            .borders(Borders::ALL)
            .border_type(if is_selected {
                BorderType::Thick
            } else {
                BorderType::Rounded
            })
            .border_style(Style::default().fg(border_color).add_modifier(Modifier::ITALIC));

        let card_content = vec![
            Line::from(""),
            Line::from(vec![Span::styled(
                "   ➕ Load more…",
                Style::default().fg(app.theme.lavender).add_modifier(Modifier::BOLD),
            )]),
        ];
        f.render_widget(Paragraph::new(card_content).block(card_block), card_area);
        app.click_map.push((card_area, ClickTarget::LyricsLoadMore));
    }

    let hint = " [↑↓←→] Navigate  [Enter/m] Load more / Select  [Esc] Back ";
    f.render_widget(
        Paragraph::new(hint).style(Style::default().bg(app.theme.surface0).fg(app.theme.subtext)),
        Rect::new(inner.x, inner.y + inner.height.saturating_sub(1), inner.width, 1),
    );
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    s.chars().take(max_chars.saturating_sub(1)).collect::<String>() + "…"
}
