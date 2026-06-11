//! Timeline service: turns `download_history` rows into a paginated,
//! date-bucketed view. Pure helpers (`bucket_for`, `media_kind_from_format`,
//! `group_into_buckets`) carry no I/O and are unit-tested directly.

use chrono::{DateTime, Utc};
use serde::Serialize;

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
}
