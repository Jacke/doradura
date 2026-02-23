//! Video preview popup: thumbnail, metadata, quality selector.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, ClickTarget, DownloadFormat, PreviewState};
use crate::theme;
use crate::video_info::{fmt_count, fmt_duration, fmt_size, ThumbnailArt, VideoInfo, THUMB_H, THUMB_W};

const SPINNER: &[&str] = &[
    "\u{28fe}", "\u{28fd}", "\u{28fb}", "\u{287f}", "\u{28bf}", "\u{289f}", "\u{28af}", "\u{28f7}",
];

// ── Public entry point ────────────────────────────────────────────────────────

pub fn render_preview_popup(f: &mut Frame, area: Rect, app: &mut App) {
    // Minimum viable size
    if area.width < 80 || area.height < 20 {
        return;
    }

    let popup_w = (THUMB_W as u16 + 52).min(area.width.saturating_sub(4));
    let popup_h = (THUMB_H as u16 + 8).min(area.height.saturating_sub(4));
    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

    let block = Block::default()
        .title(" 🎬 Video Preview ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::LAVENDER))
        .style(Style::default().bg(theme::BASE));

    let inner = block.inner(popup_area);
    f.render_widget(Clear, popup_area);
    f.render_widget(block, popup_area);

    match &app.preview_state {
        PreviewState::Hidden => {}
        PreviewState::Loading => render_loading(f, inner, app),
        PreviewState::Ready { info, .. } => {
            let info = info.clone();
            let channel_url = info.channel_url.clone();
            let thumb = app.preview_thumbnail.clone();
            let video_url = app.preview_url.clone();
            let mut cm = std::mem::take(&mut app.click_map);
            render_ready(
                f,
                inner,
                app,
                &info,
                thumb.as_ref(),
                &video_url,
                channel_url.as_deref(),
                &mut cm,
            );
            app.click_map = cm;
        }
        PreviewState::Failed(msg) => {
            let msg = msg.clone();
            render_failed(f, inner, &msg);
        }
    }
}

// ── Loading state ─────────────────────────────────────────────────────────────

fn render_loading(f: &mut Frame, area: Rect, app: &App) {
    let spinner = SPINNER[app.spinner_frame as usize % SPINNER.len()];
    let lines = vec![
        Line::from(""),
        Line::from(""),
        Line::from(vec![
            Span::styled(format!("  {}  ", spinner), Style::default().fg(theme::YELLOW)),
            Span::styled(
                "Fetching video info…",
                Style::default().fg(theme::TEXT).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            format!(
                "  {}",
                truncate_url(&app.preview_url, area.width.saturating_sub(4) as usize)
            ),
            Style::default().fg(theme::SUBTEXT),
        )),
        Line::from(""),
        Line::from(Span::styled("  [Esc] Cancel", Style::default().fg(theme::SUBTEXT))),
    ];
    f.render_widget(Paragraph::new(lines), area);
}

// ── Ready state ───────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn render_ready(
    f: &mut Frame,
    area: Rect,
    app: &App,
    info: &VideoInfo,
    thumb: Option<&ThumbnailArt>,
    video_url: &str,
    channel_url: Option<&str>,
    click_map: &mut Vec<(Rect, ClickTarget)>,
) {
    let thumb_col_w = THUMB_W as u16 + 2;

    // ── Horizontal split: thumbnail | right pane ──────────────────────────────
    let h_split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(thumb_col_w), Constraint::Min(1)])
        .split(area);

    // ── Right pane vertical split: metadata | gap | quality | hints ───────────
    let v_split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // metadata
            Constraint::Length(1), // gap
            Constraint::Length(2), // quality/format selector
            Constraint::Length(1), // hint bar
        ])
        .split(h_split[1]);

    render_thumbnail(f, h_split[0], thumb);
    render_metadata(f, v_split[0], info, video_url, channel_url, click_map);
    render_quality_row(f, v_split[2], info, app, click_map);
    render_hint_bar(f, v_split[3], app, click_map);
}

// ── Thumbnail ─────────────────────────────────────────────────────────────────

fn render_thumbnail(f: &mut Frame, area: Rect, thumb: Option<&ThumbnailArt>) {
    let inner_x = area.x + 1;
    let avail_h = area.height as usize;

    if let Some(art) = thumb {
        for (row_idx, row) in art.rows.iter().enumerate() {
            if row_idx >= avail_h {
                break;
            }
            let y = area.y + row_idx as u16;
            let spans: Vec<Span> = row
                .iter()
                .map(|(top, bot)| {
                    Span::styled(
                        "▀",
                        Style::default()
                            .fg(Color::Rgb(top[0], top[1], top[2]))
                            .bg(Color::Rgb(bot[0], bot[1], bot[2])),
                    )
                })
                .collect();
            let row_area = Rect::new(inner_x, y, area.width.saturating_sub(2), 1);
            f.render_widget(Paragraph::new(vec![Line::from(spans)]), row_area);
        }
    } else {
        // Stylized placeholder: dark panel with film-strip borders
        let w = area.width.saturating_sub(2) as usize;
        let content_h = avail_h.saturating_sub(2);
        let center_row = content_h / 2;

        // Top border
        let border_line = "─".repeat(w);
        f.render_widget(
            Paragraph::new(border_line.clone()).style(Style::default().fg(theme::SURFACE0)),
            Rect::new(inner_x, area.y, area.width.saturating_sub(2), 1),
        );

        for row in 0..content_h {
            let y = area.y + 1 + row as u16;
            let content = if row == center_row {
                format!("{:^w$}", "🎬", w = w.saturating_sub(2))
            } else {
                " ".repeat(w)
            };
            let style = if row == center_row {
                Style::default().fg(theme::LAVENDER).bg(theme::SURFACE0)
            } else {
                Style::default().bg(theme::SURFACE0)
            };
            f.render_widget(
                Paragraph::new(content).style(style),
                Rect::new(inner_x, y, area.width.saturating_sub(2), 1),
            );
        }

        // Bottom border
        f.render_widget(
            Paragraph::new(border_line).style(Style::default().fg(theme::SURFACE0)),
            Rect::new(
                inner_x,
                area.y + avail_h.saturating_sub(1) as u16,
                area.width.saturating_sub(2),
                1,
            ),
        );
    }
}

// ── Metadata panel ────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn render_metadata(
    f: &mut Frame,
    area: Rect,
    info: &VideoInfo,
    video_url: &str,
    channel_url: Option<&str>,
    click_map: &mut Vec<(Rect, ClickTarget)>,
) {
    let col_w = area.width.saturating_sub(2) as usize;

    let label = |icon: &str, text: String| -> Line {
        Line::from(vec![
            Span::styled(format!(" {icon}  "), Style::default().fg(theme::LAVENDER)),
            Span::styled(text, Style::default().fg(theme::TEXT)),
        ])
    };

    // Title (may be long — truncate)
    let title_truncated = truncate_str(&info.title, col_w.saturating_sub(2));
    let separator = "─".repeat(title_truncated.chars().count().max(12));

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            format!(" {}", title_truncated),
            Style::default().fg(theme::LAVENDER).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!(" {}", separator),
            Style::default().fg(theme::SURFACE0),
        )),
        label("👤", info.uploader.clone()),
        label("⏱ ", fmt_duration(info.duration_secs)),
    ];

    if let Some(v) = info.view_count {
        lines.push(label("👁 ", format!("{} views", fmt_count(v))));
    }
    if let Some(l) = info.like_count {
        lines.push(label("👍", format!("{} likes", fmt_count(l))));
    }
    if let Some(s) = info.filesize_approx {
        lines.push(label("💾", format!("~{}", fmt_size(s))));
    }

    // Title line (row 0 of the metadata area) → open video in browser
    click_map.push((
        Rect::new(area.x, area.y, area.width, 1),
        ClickTarget::OpenInBrowser(video_url.to_string()),
    ));
    // Uploader line (row 2, after title + separator) → open channel in browser
    if let Some(curl) = channel_url {
        click_map.push((
            Rect::new(area.x, area.y + 2, area.width, 1),
            ClickTarget::OpenInBrowser(curl.to_string()),
        ));
    }

    f.render_widget(Paragraph::new(lines), area);
}

// ── Quality / format row ──────────────────────────────────────────────────────

fn render_quality_row(
    f: &mut Frame,
    area: Rect,
    info: &VideoInfo,
    app: &App,
    click_map: &mut Vec<(Rect, ClickTarget)>,
) {
    let fmt = app.preview_format;
    let cursor = app.preview_quality_cursor;

    // Format toggle label
    let fmt_label_text = format!(" Format: {} ", fmt.label());
    let fmt_label_w = fmt_label_text.chars().count() as u16;
    let fmt_span = Span::styled(
        fmt_label_text,
        Style::default().fg(theme::PEACH).add_modifier(Modifier::BOLD),
    );

    // Register format toggle click area
    click_map.push((
        Rect::new(area.x, area.y, fmt_label_w, 1),
        ClickTarget::PreviewToggleFormat,
    ));

    if fmt == DownloadFormat::Mp3 {
        // MP3: just show format, no quality selector
        f.render_widget(
            Paragraph::new(Line::from(vec![
                fmt_span,
                Span::styled("  [Tab] switch to MP4 →", Style::default().fg(theme::SUBTEXT)),
            ])),
            Rect::new(area.x, area.y, area.width, 1),
        );
        return;
    }

    // MP4: quality selector
    let heights = quality_list(info);

    // Register each quality label click area
    let quality_prefix_w = fmt_label_w + "  Quality: ".len() as u16;
    let mut x = area.x + quality_prefix_w;
    for (i, label) in heights.iter().enumerate() {
        let w = (2 + label.len() + 1) as u16; // "▶ " + label + " "
        click_map.push((Rect::new(x, area.y, w, 1), ClickTarget::PreviewQuality(i)));
        x += w;
    }

    let mut spans: Vec<Span> = vec![fmt_span, Span::raw("  Quality: ")];
    for (i, label) in heights.iter().enumerate() {
        let is_sel = i == cursor;
        let (pre, suf) = if is_sel { ("▶ ", " ") } else { ("  ", " ") };
        let style = if is_sel {
            Style::default().fg(theme::LAVENDER).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::SUBTEXT)
        };
        spans.push(Span::styled(format!("{}{}{}", pre, label, suf), style));
    }

    f.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(area.x, area.y, area.width, 1),
    );
}

/// Build the quality option labels from the VideoInfo.
pub fn quality_list(info: &VideoInfo) -> Vec<String> {
    let mut list: Vec<String> = info.available_heights.iter().map(|h| format!("{}p", h)).collect();
    list.push("best".to_string());
    list
}

// ── Hint bar ──────────────────────────────────────────────────────────────────

fn render_hint_bar(f: &mut Frame, area: Rect, app: &App, click_map: &mut Vec<(Rect, ClickTarget)>) {
    let k = |s: &str| {
        Span::styled(
            s.to_string(),
            Style::default().fg(theme::PEACH).add_modifier(Modifier::BOLD),
        )
    };
    let d = |s: &str| Span::styled(s.to_string(), Style::default().fg(theme::SUBTEXT));
    let sep = || Span::raw("  ");

    let mp4_hints = app.preview_format == DownloadFormat::Mp4;
    let mut spans = vec![];
    if mp4_hints {
        spans.extend([k(" [←→]"), d(" Quality"), sep()]);
    }
    spans.extend([
        k("[Tab]"),
        d(" Toggle MP3/MP4"),
        sep(),
        k("[Enter]"),
        d(" Download"),
        sep(),
        k("[Esc]"),
        d(" Cancel"),
    ]);

    // Register the whole hint bar area as a Download click target
    click_map.push((area, ClickTarget::PreviewDownload));

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ── Failed state ──────────────────────────────────────────────────────────────

fn render_failed(f: &mut Frame, area: Rect, msg: &str) {
    // Split area: top for error text, bottom 2 lines for action hint
    let chunks = ratatui::layout::Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            ratatui::layout::Constraint::Min(1),
            ratatui::layout::Constraint::Length(2),
        ])
        .split(area);

    let k = |s: &str| {
        Span::styled(
            s.to_string(),
            Style::default().fg(theme::PEACH).add_modifier(Modifier::BOLD),
        )
    };
    let d = |s: &str| Span::styled(s.to_string(), Style::default().fg(theme::SUBTEXT));

    let error_lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  ✖  Could not fetch video info",
            Style::default().fg(theme::RED).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(format!("  {}", msg), Style::default().fg(theme::SUBTEXT))),
    ];

    f.render_widget(Paragraph::new(error_lines).wrap(Wrap { trim: false }), chunks[0]);

    let hint = Line::from(vec![
        k("  [Enter]"),
        d(" Download anyway    "),
        k("[Esc]"),
        d(" Cancel"),
    ]);
    f.render_widget(Paragraph::new(vec![Line::from(""), hint]), chunks[1]);
}

// ── String helpers ────────────────────────────────────────────────────────────

fn truncate_str(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let mut chars = s.chars();
    let out: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        // Take max-1 chars (safe, char-boundary-aware), then append ellipsis
        let shorter: String = out.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", shorter)
    } else {
        out
    }
}

fn truncate_url(url: &str, max: usize) -> String {
    truncate_str(url, max)
}
