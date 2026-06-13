//! Pure builders: a `TimelinePage` → message body (HTML) + inline keyboard.
//!
//! Design (the "button-per-track" layout): the body renders a rich, detailed
//! HTML **card** per download (type + platform emoji, bold artist, title, and a
//! monospace tech badge with format · quality · size · duration · time); the
//! keyboard carries one number-emoji button per card (1️⃣…🔟) that re-sends it.
//! The big number appears in both the card and its button, so the eye doesn't
//! hunt. Inline-button labels are plain text (no markup), which is why all the
//! rich detail lives in the body.
//!
//! No I/O, no Telegram API calls — fully unit-testable. `esc` (HTML escaper) and
//! `bucket_header` (localized date label) are injected to keep this pure.

use doracore::explore::timeline::{BucketLabel, MediaKind, TimelineEntry, TimelinePage};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

/// Type badge for a download.
fn media_emoji(m: MediaKind) -> &'static str {
    match m {
        MediaKind::Audio => "🎵",
        MediaKind::Video => "🎬",
        MediaKind::VideoNote => "⭕",
        MediaKind::Gif => "🎞",
        MediaKind::Other => "📄",
    }
}

/// Source-platform badge `(emoji, name)`, derived from the URL.
fn platform_badge(url: &str) -> (&'static str, &'static str) {
    match doracore::core::metrics::extract_platform(url) {
        "youtube" => ("▶️", "YouTube"),
        "soundcloud" => ("☁️", "SoundCloud"),
        "instagram" => ("📸", "Instagram"),
        "tiktok" => ("🎵", "TikTok"),
        "vimeo" => ("🎯", "Vimeo"),
        "spotify" => ("🟢", "Spotify"),
        "twitter" | "x" => ("🐦", "X"),
        "vk" => ("💬", "VK"),
        _ => ("🔗", "Web"),
    }
}

/// Keycap number emoji 1️⃣…🔟 for 1–10; bare number beyond (page size is 10).
fn number_emoji(n: u32) -> String {
    match n {
        1..=9 => format!("{n}\u{fe0f}\u{20e3}"),
        10 => "🔟".to_string(),
        _ => n.to_string(),
    }
}

/// Human-readable byte size (KB / MB / GB, 1 decimal).
fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Duration as `M:SS` (or `H:MM:SS` past an hour).
fn fmt_duration(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Pretty format label (`MP3`, `MP4`, `NOTE`, `GIF`, …).
fn format_label(fmt: &str) -> String {
    match fmt.to_lowercase().as_str() {
        "video_note" | "note" | "circle" => "NOTE".to_string(),
        other => other.to_uppercase(),
    }
}

/// Build the monospace tech badge: `FORMAT · quality · size · duration`.
fn tech_badge(e: &TimelineEntry) -> String {
    let mut parts: Vec<String> = vec![format_label(&e.format)];
    let quality = match e.media {
        MediaKind::Video => e.video_quality.clone(),
        MediaKind::Audio => e.audio_bitrate.clone(),
        _ => None,
    };
    if let Some(q) = quality.filter(|s| !s.trim().is_empty()) {
        parts.push(q);
    }
    if let Some(sz) = e.size_bytes {
        parts.push(human_size(sz));
    }
    if let Some(d) = e.duration_secs.filter(|d| *d > 0) {
        parts.push(fmt_duration(d));
    }
    parts.join(" · ")
}

/// Build the timeline message body in **HTML**. `title` is the (already escaped /
/// safe) header line; `esc` HTML-escapes user-supplied text; `bucket_header`
/// localizes a date bucket. One rich card per entry, numbered to match its button.
pub fn render_timeline_text(
    page: &TimelinePage,
    title: &str,
    empty_msg: &str,
    bucket_header: &dyn Fn(BucketLabel) -> String,
    esc: &dyn Fn(&str) -> String,
) -> String {
    if page.total_entries == 0 {
        return format!("{title}\n\n{}", esc(empty_msg));
    }
    let mut out = String::from(title);
    let mut n = 0u32;
    for bucket in &page.buckets {
        out.push_str(&format!("\n\n<b>──  {}  ──</b>\n", esc(&bucket_header(bucket.label))));
        for e in &bucket.entries {
            n += 1;
            let num = number_emoji(n);
            let title_part = esc(&e.title);
            let head = if e.artist.trim().is_empty() {
                format!("{} <b>{}</b>", media_emoji(e.media), title_part)
            } else {
                format!("{} <b>{}</b> — {}", media_emoji(e.media), esc(&e.artist), title_part)
            };
            let (pemoji, pname) = platform_badge(&e.url);
            let time = e.at.format("%H:%M");
            out.push_str(&format!(
                "\n{num}  {head}\n     └ <code>{}</code> · {pemoji} {pname} · {time}\n",
                esc(&tech_badge(e)),
            ));
        }
    }
    out
}

/// Build the inline keyboard: one number-emoji re-send button per entry (5/row),
/// then the pager, then the tab bar.
pub fn render_timeline_keyboard(
    page: &TimelinePage,
    tab_recent: &str,
    tab_trending: &str,
    tab_subs: &str,
    page_label: &str,
) -> InlineKeyboardMarkup {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    let mut num_row: Vec<InlineKeyboardButton> = Vec::new();
    let mut n = 0u32;
    for bucket in &page.buckets {
        for e in &bucket.entries {
            n += 1;
            num_row.push(crate::telegram::cb(number_emoji(n), format!("exp:rs:{}", e.id)));
            if num_row.len() == 5 {
                rows.push(std::mem::take(&mut num_row));
            }
        }
    }
    if !num_row.is_empty() {
        rows.push(num_row);
    }

    let mut pager: Vec<InlineKeyboardButton> = Vec::new();
    if page.page > 0 {
        pager.push(crate::telegram::cb("‹", format!("exp:page:recent:{}", page.page - 1)));
    }
    pager.push(crate::telegram::cb(page_label, "exp:noop".to_string()));
    if page.page + 1 < page.total_pages {
        pager.push(crate::telegram::cb("›", format!("exp:page:recent:{}", page.page + 1)));
    }
    rows.push(pager);

    rows.push(vec![
        crate::telegram::cb(tab_recent, "exp:tab:recent".to_string()),
        crate::telegram::cb(tab_trending, "exp:tab:trending".to_string()),
        crate::telegram::cb(tab_subs, "exp:tab:subs".to_string()),
    ]);

    InlineKeyboardMarkup::new(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use doracore::explore::timeline::paginate;

    fn entry(id: i64) -> TimelineEntry {
        TimelineEntry {
            id,
            title: "Song".into(),
            artist: "Art".into(),
            media: MediaKind::Audio,
            file_id: Some("F".into()),
            url: "https://youtu.be/x".into(),
            at: Utc.with_ymd_and_hms(2026, 6, 11, 9, 5, 0).unwrap(),
            format: "mp3".into(),
            size_bytes: Some(8_400_000),
            duration_secs: Some(204),
            video_quality: None,
            audio_bitrate: Some("320k".into()),
        }
    }

    #[test]
    fn renders_html_cards_with_detail() {
        let now = Utc.with_ymd_and_hms(2026, 6, 11, 12, 0, 0).unwrap();
        let page = paginate(vec![entry(1), entry(2)], 0, now);
        let text = render_timeline_text(&page, "TITLE", "EMPTY", &|_| "Сегодня".to_string(), &|s| {
            s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
        });
        assert!(text.contains("TITLE"));
        assert!(text.contains("──  Сегодня  ──"));
        // number-emoji cards
        assert!(text.contains("1\u{fe0f}\u{20e3}"));
        assert!(text.contains("2\u{fe0f}\u{20e3}"));
        // rich detail: bold artist, mono tech badge, platform, time
        assert!(text.contains("<b>Art</b>"));
        assert!(text.contains("<code>MP3 · 320k · 8.0 MB · 3:24</code>"));
        assert!(text.contains("▶️ YouTube"));
        assert!(text.contains("09:05"));
    }

    #[test]
    fn empty_page_shows_empty_message() {
        let now = Utc.with_ymd_and_hms(2026, 6, 11, 12, 0, 0).unwrap();
        let page = paginate(vec![], 0, now);
        let text = render_timeline_text(&page, "TITLE", "EMPTY", &|_| "H".to_string(), &|s| s.to_string());
        assert!(text.contains("EMPTY"));
    }

    #[test]
    fn helpers_format_correctly() {
        assert_eq!(human_size(8_400_000), "8.0 MB");
        assert_eq!(human_size(900), "900 B");
        assert_eq!(human_size(2_000), "2 KB");
        assert_eq!(fmt_duration(204), "3:24");
        assert_eq!(fmt_duration(3_661), "1:01:01");
        assert_eq!(number_emoji(1), "1\u{fe0f}\u{20e3}");
        assert_eq!(number_emoji(10), "🔟");
        assert_eq!(platform_badge("https://soundcloud.com/x").1, "SoundCloud");
    }
}
