//! Video preview popup: thumbnail, metadata, quality selector.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, ClickTarget, DownloadFormat, PreviewState};
use crate::theme::ThemeColors;
use crate::video_info::{fmt_count, fmt_duration, fmt_size, ThumbnailArt, VideoInfo, THUMB_H};

const SPINNER: &[&str] = &[
    "\u{28fe}", "\u{28fd}", "\u{28fb}", "\u{287f}", "\u{28bf}", "\u{289f}", "\u{28af}", "\u{28f7}",
];

// ── Public entry point ────────────────────────────────────────────────────────

pub fn render_preview_popup(f: &mut Frame, area: Rect, app: &mut App) {
    if area.width < 60 || area.height < 15 {
        return;
    }

    let (mut popup_w, mut popup_h) = (80_u16, 22_u16);

    if let PreviewState::Ready { .. } = &app.preview_state {
        let thumb = app.preview_thumbnail.as_ref();
        let is_vertical = thumb.is_some_and(|t| (t.height as f32 * 2.2) > t.width as f32);

        if is_vertical {
            popup_w = 74;
            popup_h = 26;
        } else {
            let thumb_w = thumb.map(|t| t.width + 4).unwrap_or(72);
            popup_w = thumb_w.max(80).min(area.width.saturating_sub(4));
            popup_h = 30; // Increased to prevent cutting
        }
    }

    let popup_w = popup_w.min(area.width.saturating_sub(2));
    let popup_h = popup_h.min(area.height.saturating_sub(2));
    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

    // Feature: Close on click (Background blocker)
    // Add this after the main UI but before popup elements to intercept clicks.
    app.click_map.push((area, ClickTarget::PreviewClose));

    let block = Block::default()
        .title(" 🎬 Video Preview ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(app.theme.lavender))
        .style(Style::default().bg(app.theme.base));

    let inner = block.inner(popup_area);
    f.render_widget(Clear, popup_area);
    f.render_widget(block, popup_area);

    let theme = app.theme;
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
                &theme,
            );
            app.click_map = cm;
        }
        PreviewState::Failed(msg) => {
            let msg = msg.clone();
            render_failed(f, inner, &msg, &theme);
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
            Span::styled(format!("  {}  ", spinner), Style::default().fg(app.theme.yellow)),
            Span::styled(
                "Fetching video info…",
                Style::default().fg(app.theme.text).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            format!(
                "  {}",
                truncate_url(&app.preview_url, area.width.saturating_sub(4) as usize)
            ),
            Style::default().fg(app.theme.subtext),
        )),
        Line::from(""),
        Line::from(Span::styled("  [Esc] Cancel", Style::default().fg(app.theme.subtext))),
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
    theme: &ThemeColors,
) {
    // Decide layout based on the actual thumbnail we have.
    // If thumbnail is wider than high (including character aspect ratio correction), use horizontal layout.
    let is_vertical = thumb.is_some_and(|t| (t.height as f32 * 2.2) > t.width as f32);

    if is_vertical {
        // Feature: Shorts Layout (Vertical media)
        let thumb_w = thumb.map(|t| t.width + 2).unwrap_or(26);
        let h_split = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(thumb_w.min(area.width / 2)), Constraint::Min(30)])
            .split(area);

        render_thumbnail(f, h_split[0], thumb, theme, app);

        let right_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),    // metadata
                Constraint::Length(2), // quality/format
                Constraint::Length(1), // hint
            ])
            .split(h_split[1]);

        render_metadata(f, right_chunks[0], info, video_url, channel_url, click_map, theme);
        render_quality_row(f, right_chunks[1], info, app, click_map);
        render_hint_bar(f, right_chunks[2], app, click_map);
    } else {
        // Feature: Video Layout (Horizontal media)
        let thumb_h = thumb.map(|t| t.height).unwrap_or(THUMB_H as u16);
        let v_split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(thumb_h),
                Constraint::Min(6),    // metadata
                Constraint::Length(2), // quality/format
                Constraint::Length(1), // hint
            ])
            .split(area);

        render_thumbnail(f, v_split[0], thumb, theme, app);
        render_metadata(f, v_split[1], info, video_url, channel_url, click_map, theme);
        render_quality_row(f, v_split[2], info, app, click_map);
        render_hint_bar(f, v_split[3], app, click_map);
    }
}

// ── Thumbnail ─────────────────────────────────────────────────────────────────

pub fn render_thumbnail(f: &mut Frame, area: Rect, thumb: Option<&ThumbnailArt>, theme: &ThemeColors, app: &App) {
    let avail_h = area.height as usize;

    if let Some(art) = thumb {
        // Calculate centered x offset
        let thumb_w = art.width;
        let x_offset = area.width.saturating_sub(thumb_w) / 2;
        let centered_area = Rect::new(area.x + x_offset, area.y, thumb_w.min(area.width), area.height);

        // Feature: High-quality Image Preview (Kitty/Sixel/iTerm2)
        if let Some(protocol) = &app.preview_image_protocol {
            let image_widget = ratatui_image::Image::new(protocol);
            // We still use the full area for the widget but it should ideally be centered.
            // Some protocols handle centering better if given the precise area.
            f.render_widget(image_widget, centered_area);
            return;
        }

        // Fallback: ASCII half-block rendering
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
            let row_area = Rect::new(area.x + x_offset, y, thumb_w.min(area.width), 1);
            f.render_widget(Paragraph::new(vec![Line::from(spans)]), row_area);
        }
    } else {
        let inner_x = area.x + 1;
        let w = area.width.saturating_sub(2) as usize;
        let content_h = avail_h.saturating_sub(2);
        let center_row = content_h / 2;

        let border_line = "─".repeat(w);
        f.render_widget(
            Paragraph::new(border_line.clone()).style(Style::default().fg(theme.surface0)),
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
                Style::default().fg(theme.lavender).bg(theme.surface0)
            } else {
                Style::default().bg(theme.surface0)
            };
            f.render_widget(
                Paragraph::new(content).style(style),
                Rect::new(inner_x, y, area.width.saturating_sub(2), 1),
            );
        }

        f.render_widget(
            Paragraph::new(border_line).style(Style::default().fg(theme.surface0)),
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
    theme: &ThemeColors,
) {
    let col_w = area.width.saturating_sub(2) as usize;

    let label = |icon: &str, text: String| -> Line {
        Line::from(vec![
            Span::styled(format!(" {icon}  "), Style::default().fg(theme.lavender)),
            Span::styled(text, Style::default().fg(theme.text)),
        ])
    };

    let title_truncated = truncate_str(&info.title, col_w.saturating_sub(2));
    let separator = "─".repeat(title_truncated.chars().count().max(12));

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            format!(" {}", title_truncated),
            Style::default().fg(theme.lavender).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!(" {}", separator),
            Style::default().fg(theme.surface0),
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

    click_map.push((
        Rect::new(area.x, area.y, area.width, 1),
        ClickTarget::OpenInBrowser(video_url.to_string()),
    ));
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
    // When subtitle menu is open, render it instead of quality row
    if app.preview_subs_menu {
        render_subs_menu(f, area, info, app, click_map);
        return;
    }

    let fmt = app.preview_format;
    let cursor = app.preview_quality_cursor;

    let fmt_label_text = format!(" Format: {} ", fmt.label());
    let fmt_label_w = fmt_label_text.chars().count() as u16;
    let fmt_span = Span::styled(
        fmt_label_text,
        Style::default().fg(app.theme.peach).add_modifier(Modifier::BOLD),
    );

    click_map.push((
        Rect::new(area.x, area.y, fmt_label_w, 1),
        ClickTarget::PreviewToggleFormat,
    ));

    if fmt == DownloadFormat::Mp3 {
        f.render_widget(
            Paragraph::new(Line::from(vec![
                fmt_span,
                Span::styled("  [Tab] switch to MP4 →", Style::default().fg(app.theme.subtext)),
            ])),
            Rect::new(area.x, area.y, area.width, 1),
        );
        return;
    }

    let heights = quality_list(info);

    let quality_prefix_w = fmt_label_w + "  Quality: ".len() as u16;
    let mut x = area.x + quality_prefix_w;
    for (i, label) in heights.iter().enumerate() {
        let w = (2 + label.len() + 1) as u16;
        click_map.push((Rect::new(x, area.y, w, 1), ClickTarget::PreviewQuality(i)));
        x += w;
    }

    let mut spans: Vec<Span> = vec![fmt_span, Span::raw("  Quality: ")];
    for (i, label) in heights.iter().enumerate() {
        let is_sel = i == cursor;
        let (pre, suf) = if is_sel { ("▶ ", " ") } else { ("  ", " ") };
        let style = if is_sel {
            Style::default().fg(app.theme.lavender).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.subtext)
        };
        spans.push(Span::styled(format!("{}{}{}", pre, label, suf), style));
    }

    // [SRT] / [SRT ✓] button — greyed out when no subtitles available
    if info.subtitle_langs.is_empty() {
        spans.push(Span::styled(" [SRT] ", Style::default().fg(app.theme.surface0)));
    } else {
        let srt_label = if app.preview_subs_enabled {
            " [SRT ✓] "
        } else {
            " [SRT] "
        };
        let srt_style = if app.preview_subs_enabled {
            Style::default().fg(app.theme.green).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.subtext)
        };
        let srt_w = srt_label.len() as u16;
        click_map.push((Rect::new(x, area.y, srt_w, 1), ClickTarget::PreviewToggleSubsMenu));
        spans.push(Span::styled(srt_label.to_string(), srt_style));
    }

    f.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(area.x, area.y, area.width, 1),
    );
}

// ── Subtitle menu ─────────────────────────────────────────────────────────────

fn render_subs_menu(f: &mut Frame, area: Rect, info: &VideoInfo, app: &App, click_map: &mut Vec<(Rect, ClickTarget)>) {
    let theme = &app.theme;

    if app.preview_subs_editing {
        // Custom language input mode
        let mut spans = vec![
            Span::styled(" Language: ", Style::default().fg(theme.lavender)),
            Span::styled(
                &app.preview_subs_edit_buf,
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
        ];
        if app.blink_on {
            spans.push(Span::styled("▌", Style::default().fg(theme.lavender)));
        }
        spans.push(Span::styled(
            "  [Enter] Confirm  [Esc] Cancel",
            Style::default().fg(theme.subtext),
        ));
        f.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(area.x, area.y, area.width, 1),
        );
        return;
    }

    // Line 1: Toggle + language list
    let mut spans: Vec<Span> = vec![Span::styled(" Subtitles: ", Style::default().fg(theme.lavender))];

    // ON / OFF toggle
    let (on_style, off_style) = if app.preview_subs_enabled {
        (
            Style::default().fg(theme.green).add_modifier(Modifier::BOLD),
            Style::default().fg(theme.subtext),
        )
    } else {
        (
            Style::default().fg(theme.subtext),
            Style::default().fg(theme.red).add_modifier(Modifier::BOLD),
        )
    };
    let on_label = if app.preview_subs_enabled { "● ON " } else { "○ ON " };
    let off_label = if app.preview_subs_enabled { "○ OFF" } else { "● OFF" };

    let toggle_x = area.x + " Subtitles: ".len() as u16;
    click_map.push((
        Rect::new(toggle_x, area.y, 10, 1),
        ClickTarget::PreviewToggleSubsEnabled,
    ));

    spans.push(Span::styled(on_label, on_style));
    spans.push(Span::styled(off_label, off_style));
    spans.push(Span::raw("  "));

    // Language list (show up to ~8 languages that fit)
    if app.preview_subs_enabled {
        spans.push(Span::styled("Lang: ", Style::default().fg(theme.lavender)));
        let max_langs = 10.min(info.subtitle_langs.len());
        let lang_x_start = area.x + spans.iter().map(|s| s.width()).sum::<usize>() as u16;
        let mut lx = lang_x_start;

        // Determine selected lang: custom overrides cursor
        let selected_idx = if app.preview_subs_custom_lang.is_some() {
            None // custom lang selected, no index highlighted
        } else {
            Some(app.preview_subs_lang_cursor)
        };

        for (i, lang) in info.subtitle_langs.iter().take(max_langs).enumerate() {
            let is_sel = selected_idx == Some(i);
            let style = if is_sel {
                Style::default().fg(theme.lavender).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.subtext)
            };
            let pre = if is_sel { "▶" } else { " " };
            let label = format!("{}{} ", pre, lang);
            let w = label.len() as u16;
            click_map.push((Rect::new(lx, area.y, w, 1), ClickTarget::PreviewSubsLang(i)));
            spans.push(Span::styled(label, style));
            lx += w;
        }

        // [custom] button
        if let Some(ref custom) = app.preview_subs_custom_lang {
            spans.push(Span::styled(
                format!(" [{}]", custom),
                Style::default().fg(theme.yellow).add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(" [custom]", Style::default().fg(theme.subtext)));
        }
    }

    f.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(area.x, area.y, area.width, 1),
    );

    // Line 2: hint bar for subtitle menu
    if area.height > 1 {
        let k = |s: &str| {
            Span::styled(
                s.to_string(),
                Style::default().fg(theme.peach).add_modifier(Modifier::BOLD),
            )
        };
        let d = |s: &str| Span::styled(s.to_string(), Style::default().fg(theme.subtext));
        let sep = || Span::raw("  ");
        let hints = vec![
            k(" [←→]"),
            d(" Language"),
            sep(),
            k("[Space]"),
            d(" Toggle"),
            sep(),
            k("[c]"),
            d(" Custom"),
            sep(),
            k("[Esc]"),
            d(" Back"),
        ];
        f.render_widget(
            Paragraph::new(Line::from(hints)),
            Rect::new(area.x, area.y + 1, area.width, 1),
        );
    }
}

/// Build the quality option labels from the VideoInfo.
pub fn quality_list(info: &VideoInfo) -> Vec<String> {
    let mut list: Vec<String> = info.available_heights.iter().map(|h| format!("{}p", h)).collect();
    list.push("best".to_string());
    list
}

// ── Hint bar ──────────────────────────────────────────────────────────────────

fn render_hint_bar(f: &mut Frame, area: Rect, app: &App, click_map: &mut Vec<(Rect, ClickTarget)>) {
    // When subtitle menu is open, its own hint bar is rendered by render_subs_menu
    if app.preview_subs_menu {
        return;
    }

    let k = |s: &str| {
        Span::styled(
            s.to_string(),
            Style::default().fg(app.theme.peach).add_modifier(Modifier::BOLD),
        )
    };
    let d = |s: &str| Span::styled(s.to_string(), Style::default().fg(app.theme.subtext));
    let sep = || Span::raw("  ");

    let mp4_hints = app.preview_format == DownloadFormat::Mp4;
    let mut spans = vec![];
    if mp4_hints {
        spans.extend([k(" [←→]"), d(" Quality"), sep()]);
    }
    spans.extend([k("[Tab]"), d(" Toggle MP3/MP4"), sep()]);
    if mp4_hints {
        spans.extend([k("[S]"), d(" Subs"), sep()]);
    }
    spans.extend([k("[Enter]"), d(" Download"), sep(), k("[Esc]"), d(" Cancel")]);

    click_map.push((area, ClickTarget::PreviewDownload));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ── Failed state ──────────────────────────────────────────────────────────────

fn render_failed(f: &mut Frame, area: Rect, msg: &str, theme: &ThemeColors) {
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
            Style::default().fg(theme.peach).add_modifier(Modifier::BOLD),
        )
    };
    let d = |s: &str| Span::styled(s.to_string(), Style::default().fg(theme.subtext));

    let error_lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  ✖  Could not fetch video info",
            Style::default().fg(theme.red).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(format!("  {}", msg), Style::default().fg(theme.subtext))),
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
        let shorter: String = out.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", shorter)
    } else {
        out
    }
}

fn truncate_url(url: &str, max: usize) -> String {
    truncate_str(url, max)
}
