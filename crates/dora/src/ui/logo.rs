//! Animated DORA ASCII logo with colour-sweep animation and clickable colour schemes.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, ClickTarget, LogoScheme};
use crate::theme;

const LOGO_LINES: &[&str] = &[
    "  \u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2563}  \u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2563} \u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2563}  \u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2563} ",
    "  \u{2588}\u{2588}\u{2554}\u{2550}\u{2550}\u{2588}\u{2588}\u{2563}\u{2588}\u{2588}\u{2554}\u{2550}\u{2550}\u{2550}\u{2588}\u{2588}\u{2563}\u{2588}\u{2588}\u{2554}\u{2550}\u{2550}\u{2588}\u{2588}\u{2563}\u{2588}\u{2588}\u{2554}\u{2550}\u{2550}\u{2588}\u{2588}\u{2563}",
    "  \u{2588}\u{2588}\u{2551}  \u{2588}\u{2588}\u{2551}\u{2588}\u{2588}\u{2551}   \u{2588}\u{2588}\u{2551}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2554}\u{255d}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2551}",
    "  \u{2588}\u{2588}\u{2551}  \u{2588}\u{2588}\u{2551}\u{2588}\u{2588}\u{2551}   \u{2588}\u{2588}\u{2551}\u{2588}\u{2588}\u{2554}\u{2550}\u{2550}\u{2588}\u{2588}\u{2563}\u{2588}\u{2588}\u{2554}\u{2550}\u{2550}\u{2588}\u{2588}\u{2551}",
    "  \u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2554}\u{255d}\u{255a}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2554}\u{255d}\u{2588}\u{2588}\u{2551}  \u{2588}\u{2588}\u{2551}\u{2588}\u{2588}\u{2551}  \u{2588}\u{2588}\u{2551}",
    "  \u{255a}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{255d}  \u{255a}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{255d} \u{255a}\u{2550}\u{255d}  \u{255a}\u{2550}\u{255d}\u{255a}\u{2550}\u{255d}  \u{255a}\u{2550}\u{255d}",
];

// ── Colour palettes per scheme ────────────────────────────────────────────────

const PALETTE_CATPPUCCIN: &[Color] = &[
    theme::MAUVE,
    theme::LAVENDER,
    theme::BLUE,
    theme::TEAL,
    theme::GREEN,
    theme::YELLOW,
    theme::PEACH,
    theme::RED,
];

const PALETTE_FIRE: &[Color] = &[
    Color::Rgb(255, 30, 0),    // deep red
    Color::Rgb(255, 80, 0),    // red-orange
    Color::Rgb(255, 140, 0),   // orange
    Color::Rgb(255, 200, 0),   // golden yellow
    Color::Rgb(255, 240, 80),  // bright yellow
    Color::Rgb(255, 255, 180), // hot white
    Color::Rgb(255, 200, 0),   // golden yellow (mirror)
    Color::Rgb(255, 80, 0),    // back to orange
];

const PALETTE_ICE: &[Color] = &[
    Color::Rgb(100, 180, 255), // ice blue
    Color::Rgb(140, 210, 255), // sky blue
    Color::Rgb(200, 235, 255), // pale blue
    Color::Rgb(240, 250, 255), // near white
    Color::Rgb(180, 210, 255), // cool lavender
    Color::Rgb(120, 170, 240), // deep ice
    Color::Rgb(200, 235, 255), // pale blue
    Color::Rgb(160, 220, 255), // mid blue
];

const PALETTE_MATRIX: &[Color] = &[
    Color::Rgb(0, 20, 0),      // almost black
    Color::Rgb(0, 60, 0),      // dark green
    Color::Rgb(0, 120, 0),     // medium green
    Color::Rgb(0, 200, 0),     // bright green
    Color::Rgb(0, 255, 0),     // pure green
    Color::Rgb(140, 255, 140), // pale green
    Color::Rgb(0, 200, 0),     // bright green
    Color::Rgb(0, 100, 0),     // medium dark
];

const PALETTE_SUNSET: &[Color] = &[
    Color::Rgb(255, 60, 60),  // warm red
    Color::Rgb(255, 100, 50), // orange-red
    Color::Rgb(255, 160, 50), // orange
    Color::Rgb(220, 80, 160), // pink-purple
    Color::Rgb(160, 60, 200), // purple
    Color::Rgb(100, 60, 240), // deep purple-blue
    Color::Rgb(200, 80, 150), // rose
    Color::Rgb(255, 120, 60), // peach-orange
];

const PALETTE_NEON: &[Color] = &[
    Color::Rgb(255, 0, 255),  // magenta
    Color::Rgb(0, 255, 255),  // cyan
    Color::Rgb(0, 255, 0),    // green
    Color::Rgb(255, 255, 0),  // yellow
    Color::Rgb(255, 0, 100),  // hot pink
    Color::Rgb(100, 0, 255),  // electric violet
    Color::Rgb(0, 200, 255),  // sky cyan
    Color::Rgb(255, 80, 255), // orchid
];

const PALETTE_GOLD: &[Color] = &[
    Color::Rgb(255, 215, 0),   // gold
    Color::Rgb(255, 180, 0),   // dark gold
    Color::Rgb(255, 245, 120), // light gold
    Color::Rgb(255, 255, 200), // pale gold
    Color::Rgb(220, 160, 0),   // deep amber
    Color::Rgb(255, 230, 80),  // bright gold
    Color::Rgb(200, 140, 0),   // burnished gold
    Color::Rgb(255, 200, 60),  // warm gold
];

fn palette(scheme: LogoScheme) -> &'static [Color] {
    match scheme {
        LogoScheme::Catppuccin => PALETTE_CATPPUCCIN,
        LogoScheme::Fire => PALETTE_FIRE,
        LogoScheme::Ice => PALETTE_ICE,
        LogoScheme::Matrix => PALETTE_MATRIX,
        LogoScheme::Sunset => PALETTE_SUNSET,
        LogoScheme::Neon => PALETTE_NEON,
        LogoScheme::Gold => PALETTE_GOLD,
    }
}

// ── Renderer ─────────────────────────────────────────────────────────────────

/// Render the animated logo into `area` and register a logo click target.
pub fn render_logo(f: &mut Frame, area: Rect, app: &mut App) {
    let colors = palette(app.logo_scheme);
    let n = colors.len();
    let frame = app.logo_frame as usize;
    let burst = app.logo_burst as usize;

    // Register the whole logo area as a click target (before rendering).
    app.click_map.push((area, ClickTarget::LogoClick));

    let logo_lines: Vec<Line> = LOGO_LINES
        .iter()
        .enumerate()
        .map(|(row, text)| {
            let spans: Vec<Span> = text
                .chars()
                .enumerate()
                .map(|(col, ch)| {
                    let color = if burst > 0 {
                        // ── Burst mode: each character spins independently ──────
                        // Speed starts high and decays proportionally to burst.
                        let speed = 1 + burst / 3; // 1..~34
                        let idx = (col * 5 + row * 11 + frame * speed) % n;
                        colors[idx]
                    } else {
                        // ── Normal sweep: gentle left-to-right colour wash ──────
                        let idx = (col + row * 3 + frame / 6) % n;
                        colors[idx]
                    };
                    Span::styled(ch.to_string(), Style::default().fg(color).add_modifier(Modifier::BOLD))
                })
                .collect();
            Line::from(spans)
        })
        .collect();

    // Tagline: flashes with scheme colours during burst, then settles to SUBTEXT.
    let tagline_style = if burst > 20 {
        let idx = (frame / 2) % n;
        Style::default().fg(colors[idx]).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::SUBTEXT)
    };

    let mut all_lines = logo_lines;
    all_lines.push(Line::from(""));
    all_lines.push(Line::from(Span::styled(app.logo_scheme.tagline(), tagline_style)));

    let logo = Paragraph::new(all_lines).alignment(ratatui::layout::Alignment::Center);
    f.render_widget(logo, area);
}
