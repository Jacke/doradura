//! Pure builders: a `TimelinePage` → message text + inline keyboard. No I/O,
//! no Telegram API calls — fully unit-testable.

use doracore::explore::timeline::{BucketLabel, MediaKind, TimelinePage};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

fn media_emoji(m: MediaKind) -> &'static str {
    match m {
        MediaKind::Audio => "🎵",
        MediaKind::Video => "🎬",
        MediaKind::VideoNote => "⭕",
        MediaKind::Gif => "🎞",
        MediaKind::Other => "📄",
    }
}

/// Build the timeline message body (MarkdownV2). `bucket_header` maps a
/// `BucketLabel` to a localized header; `esc` escapes user text for MarkdownV2.
/// Both are injected so this function stays pure.
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
    let mut out = format!("{title}\n");
    let mut n = 0u32;
    for bucket in &page.buckets {
        out.push_str(&format!("\n{}\n", bucket_header(bucket.label)));
        for e in &bucket.entries {
            n += 1;
            let artist = if e.artist.trim().is_empty() {
                String::new()
            } else {
                format!("{} — ", esc(&e.artist))
            };
            out.push_str(&format!(
                " {n}\\. {} {}{} · {}\n",
                media_emoji(e.media),
                artist,
                esc(&e.title),
                esc(e.media.media_label())
            ));
        }
    }
    out
}

/// Build the inline keyboard: numbered resend buttons (by history id) + pager + tabs.
pub fn render_timeline_keyboard(
    page: &TimelinePage,
    tab_recent: &str,
    tab_trending: &str,
    tab_subs: &str,
    page_label: &str,
) -> InlineKeyboardMarkup {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    // Numbered resend row(s): one button per visible entry, 5 per row.
    let mut num_row: Vec<InlineKeyboardButton> = Vec::new();
    let mut n = 0u32;
    for bucket in &page.buckets {
        for e in &bucket.entries {
            n += 1;
            num_row.push(crate::telegram::cb(format!("{n}"), format!("exp:rs:{}", e.id)));
            if num_row.len() == 5 {
                rows.push(std::mem::take(&mut num_row));
            }
        }
    }
    if !num_row.is_empty() {
        rows.push(num_row);
    }

    // Pager: ‹  page X/Y  ›
    let mut pager: Vec<InlineKeyboardButton> = Vec::new();
    if page.page > 0 {
        pager.push(crate::telegram::cb("‹", format!("exp:page:recent:{}", page.page - 1)));
    }
    pager.push(crate::telegram::cb(page_label, "exp:noop".to_string()));
    if page.page + 1 < page.total_pages {
        pager.push(crate::telegram::cb("›", format!("exp:page:recent:{}", page.page + 1)));
    }
    rows.push(pager);

    // Tabs
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
    use doracore::explore::timeline::{TimelineEntry, paginate};

    fn entry(id: i64) -> TimelineEntry {
        TimelineEntry {
            id,
            title: "Song".into(),
            artist: "Art".into(),
            media: MediaKind::Audio,
            file_id: Some("F".into()),
            url: "u".into(),
            at: Utc.with_ymd_and_hms(2026, 6, 11, 9, 0, 0).unwrap(),
        }
    }

    #[test]
    fn renders_numbered_rows_and_header() {
        let now = Utc.with_ymd_and_hms(2026, 6, 11, 12, 0, 0).unwrap();
        let page = paginate(vec![entry(1), entry(2)], 0, now);
        let text = render_timeline_text(&page, "TITLE", "EMPTY", &|_| "HEADER".to_string(), &|s| s.to_string());
        assert!(text.contains("HEADER"));
        assert!(text.contains(" 1\\. 🎵"));
        assert!(text.contains(" 2\\. 🎵"));
    }

    #[test]
    fn empty_page_shows_empty_message() {
        let now = Utc.with_ymd_and_hms(2026, 6, 11, 12, 0, 0).unwrap();
        let page = paginate(vec![], 0, now);
        let text = render_timeline_text(&page, "TITLE", "EMPTY", &|_| "H".to_string(), &|s| s.to_string());
        assert!(text.contains("EMPTY"));
    }
}
