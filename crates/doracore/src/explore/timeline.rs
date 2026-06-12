//! Timeline service: turns `download_history` rows into a paginated,
//! date-bucketed view. Pure helpers (`bucket_for`, `media_kind_from_format`,
//! `group_into_buckets`) carry no I/O and are unit-tested directly.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::storage::SharedStorage;

/// Page size for one inline timeline page. The Mini App may request more.
pub const TIMELINE_PAGE_SIZE: usize = 10;

/// Media kind, derived from the history `format` column. Drives the row emoji
/// and which `send_*` method re-send uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MediaKind {
    Audio,
    Video,
    VideoNote,
    Gif,
    Other,
}

impl MediaKind {
    /// Short label for captions, e.g. "mp3"/"mp4"/"note"/"gif".
    pub fn media_label(self) -> &'static str {
        match self {
            MediaKind::Audio => "mp3",
            MediaKind::Video => "mp4",
            MediaKind::VideoNote => "note",
            MediaKind::Gif => "gif",
            MediaKind::Other => "file",
        }
    }
}

/// Coarse, locale-independent date bucket. The renderer maps it to a localized
/// header so the inline UI and the Mini App localize independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum BucketLabel {
    Today,
    Yesterday,
    ThisWeek,
    ThisMonth,
    Earlier,
}

/// One downloaded item in render-ready, platform-neutral form.
#[derive(Debug, Clone, Serialize)]
pub struct TimelineEntry {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub media: MediaKind,
    /// `Some` → instant re-send via Telegram file_id. `None` → re-download `url`.
    pub file_id: Option<String>,
    pub url: String,
    pub at: DateTime<Utc>,
}

/// One date group, e.g. "Today".
#[derive(Debug, Clone, Serialize)]
pub struct TimelineBucket {
    pub label: BucketLabel,
    pub entries: Vec<TimelineEntry>,
}

/// A paginated, date-bucketed view of one user's downloads.
#[derive(Debug, Clone, Serialize)]
pub struct TimelinePage {
    pub buckets: Vec<TimelineBucket>,
    pub page: u32,
    pub total_pages: u32,
    pub total_entries: u32,
}

/// Map a history `format` string to a `MediaKind`.
pub fn media_kind_from_format(format: &str) -> MediaKind {
    match format.trim().to_lowercase().as_str() {
        "mp3" | "m4a" | "m4r" | "opus" | "audio" => MediaKind::Audio,
        "mp4" | "mkv" | "webm" | "video" => MediaKind::Video,
        "video_note" | "circle" | "note" => MediaKind::VideoNote,
        "gif" => MediaKind::Gif,
        _ => MediaKind::Other,
    }
}

/// Assign a UTC instant to its bucket relative to `now`. Buckets compare on the
/// calendar day (UTC): same day = Today, day-1 = Yesterday, within 7 days =
/// ThisWeek, within 31 days = ThisMonth, else Earlier.
pub fn bucket_for(at: DateTime<Utc>, now: DateTime<Utc>) -> BucketLabel {
    let days = (now.date_naive() - at.date_naive()).num_days();
    match days {
        d if d <= 0 => BucketLabel::Today,
        1 => BucketLabel::Yesterday,
        2..=6 => BucketLabel::ThisWeek,
        7..=30 => BucketLabel::ThisMonth,
        _ => BucketLabel::Earlier,
    }
}

/// Fold DESC-ordered entries into consecutive date buckets. Assumes `entries`
/// is already sorted newest-first (as the history accessor returns it), so a
/// single pass yields buckets in display order without sorting.
pub fn group_into_buckets(entries: &[TimelineEntry], now: DateTime<Utc>) -> Vec<TimelineBucket> {
    let mut buckets: Vec<TimelineBucket> = Vec::new();
    for entry in entries {
        let label = bucket_for(entry.at, now);
        match buckets.last_mut() {
            Some(b) if b.label == label => b.entries.push(entry.clone()),
            _ => buckets.push(TimelineBucket {
                label,
                entries: vec![entry.clone()],
            }),
        }
    }
    buckets
}

/// Slice `all` (DESC) into page `page`, clamping out-of-range pages to the last
/// page, and bucket that page. Pure — `build_timeline_page` feeds it DB rows.
pub fn paginate(all: Vec<TimelineEntry>, page: u32, now: DateTime<Utc>) -> TimelinePage {
    let total_entries = all.len() as u32;
    let total_pages = all.len().div_ceil(TIMELINE_PAGE_SIZE).max(1) as u32;
    let page = page.min(total_pages - 1);
    let start = (page as usize) * TIMELINE_PAGE_SIZE;
    let slice: Vec<TimelineEntry> = all.into_iter().skip(start).take(TIMELINE_PAGE_SIZE).collect();
    TimelinePage {
        buckets: group_into_buckets(&slice, now),
        page,
        total_pages,
        total_entries,
    }
}

/// Build page `page` (0-based) of `user_id`'s download timeline. `now` is
/// injected for deterministic tests.
pub async fn build_timeline_page(
    storage: &SharedStorage,
    user_id: i64,
    page: u32,
    now: DateTime<Utc>,
) -> anyhow::Result<TimelinePage> {
    let rows = storage
        .get_download_history_filtered(user_id, None, None, None, None)
        .await?;
    let entries: Vec<TimelineEntry> = rows
        .into_iter()
        .filter_map(|r| {
            let at = parse_history_timestamp(&r.downloaded_at)?;
            Some(TimelineEntry {
                id: r.id,
                title: r.title,
                artist: r.author.unwrap_or_default(),
                media: media_kind_from_format(&r.format),
                file_id: r.file_id,
                url: r.url,
                at,
            })
        })
        .collect();
    Ok(paginate(entries, page, now))
}

/// Parse the history `downloaded_at` text column. Postgres `::text` renders
/// timestamptz as RFC3339-ish; SQLite stores `YYYY-MM-DD HH:MM:SS`. Try both,
/// returning `None` for an unparseable value so the caller skips that row.
fn parse_history_timestamp(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    // Common SQLite/Postgres plain format (no tz) — assume UTC.
    for fmt in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(DateTime::from_naive_utc_and_offset(naive, Utc));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_kind_maps_known_formats() {
        assert_eq!(media_kind_from_format("mp3"), MediaKind::Audio);
        assert_eq!(media_kind_from_format("MP4"), MediaKind::Video);
        assert_eq!(media_kind_from_format("video_note"), MediaKind::VideoNote);
        assert_eq!(media_kind_from_format("gif"), MediaKind::Gif);
        assert_eq!(media_kind_from_format("srt"), MediaKind::Other);
    }

    #[test]
    fn bucket_for_classifies_relative_to_now() {
        use chrono::TimeZone;
        let now = Utc.with_ymd_and_hms(2026, 6, 11, 12, 0, 0).unwrap();
        let today = Utc.with_ymd_and_hms(2026, 6, 11, 1, 0, 0).unwrap();
        let yesterday = Utc.with_ymd_and_hms(2026, 6, 10, 23, 0, 0).unwrap();
        let three_days = Utc.with_ymd_and_hms(2026, 6, 8, 9, 0, 0).unwrap();
        let twenty_days = Utc.with_ymd_and_hms(2026, 5, 25, 9, 0, 0).unwrap();
        let old = Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap();

        assert_eq!(bucket_for(today, now), BucketLabel::Today);
        assert_eq!(bucket_for(yesterday, now), BucketLabel::Yesterday);
        assert_eq!(bucket_for(three_days, now), BucketLabel::ThisWeek);
        assert_eq!(bucket_for(twenty_days, now), BucketLabel::ThisMonth);
        assert_eq!(bucket_for(old, now), BucketLabel::Earlier);
    }

    #[test]
    fn group_into_buckets_preserves_desc_order_and_groups() {
        use chrono::TimeZone;
        let now = Utc.with_ymd_and_hms(2026, 6, 11, 12, 0, 0).unwrap();
        let mk = |id: i64, at: DateTime<Utc>| TimelineEntry {
            id,
            title: format!("t{id}"),
            artist: "a".into(),
            media: MediaKind::Audio,
            file_id: None,
            url: "u".into(),
            at,
        };
        let entries = vec![
            mk(1, Utc.with_ymd_and_hms(2026, 6, 11, 9, 0, 0).unwrap()), // Today
            mk(2, Utc.with_ymd_and_hms(2026, 6, 11, 8, 0, 0).unwrap()), // Today
            mk(3, Utc.with_ymd_and_hms(2026, 6, 10, 8, 0, 0).unwrap()), // Yesterday
        ];
        let buckets = group_into_buckets(&entries, now);
        assert_eq!(buckets.len(), 2);
        assert_eq!(buckets[0].label, BucketLabel::Today);
        assert_eq!(buckets[0].entries.len(), 2);
        assert_eq!(buckets[1].label, BucketLabel::Yesterday);
        assert_eq!(buckets[1].entries.len(), 1);
    }

    #[test]
    fn paginate_computes_meta_and_slices() {
        use chrono::TimeZone;
        let now = Utc.with_ymd_and_hms(2026, 6, 11, 12, 0, 0).unwrap();
        let all: Vec<TimelineEntry> = (0..23)
            .map(|i| TimelineEntry {
                id: i,
                title: format!("t{i}"),
                artist: "a".into(),
                media: MediaKind::Audio,
                file_id: None,
                url: "u".into(),
                at: Utc.with_ymd_and_hms(2026, 6, 11, 9, 0, 0).unwrap(),
            })
            .collect();

        let p0 = paginate(all.clone(), 0, now);
        assert_eq!(p0.total_entries, 23);
        assert_eq!(p0.total_pages, 3); // ceil(23/10)
        assert_eq!(p0.page, 0);
        assert_eq!(p0.buckets.iter().map(|b| b.entries.len()).sum::<usize>(), 10);

        let p2 = paginate(all.clone(), 2, now);
        assert_eq!(p2.page, 2);
        assert_eq!(p2.buckets.iter().map(|b| b.entries.len()).sum::<usize>(), 3); // last page

        let clamped = paginate(all, 99, now); // out of range clamps to last
        assert_eq!(clamped.page, 2);
    }
}
