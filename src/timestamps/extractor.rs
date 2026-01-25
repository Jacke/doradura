//! Combined timestamp extractor
//!
//! Extracts timestamps from all sources (URL, chapters, description)
//! and merges them into a deduplicated, sorted list.

use super::{
    chapter_parser::parse_chapters, description_parser::parse_description_timestamps, url_parser::parse_url_timestamp,
    TimestampSource, VideoTimestamp,
};
use serde_json::Value;
use url::Url;

/// Extract all timestamps from available sources
///
/// Priority:
/// 1. URL parameter timestamp (highest priority, always included)
/// 2. yt-dlp chapters (official YouTube chapters)
/// 3. Description timestamps (only if no chapters found)
///
/// Results are sorted by time and deduplicated (timestamps within 3 seconds
/// of each other are merged, keeping the one with a label).
///
/// # Arguments
///
/// * `url` - The video URL to extract timestamp parameter from
/// * `json_metadata` - Optional yt-dlp JSON metadata for chapters/description
///
/// # Returns
///
/// A vector of `VideoTimestamp` sorted by time
pub fn extract_all_timestamps(url: &Url, json_metadata: Option<&Value>) -> Vec<VideoTimestamp> {
    let mut timestamps = Vec::new();

    // 1. URL timestamp (always include if present)
    if let Some(url_time) = parse_url_timestamp(url) {
        timestamps.push(VideoTimestamp {
            source: TimestampSource::Url,
            time_seconds: url_time,
            end_seconds: None,
            label: Some("URL timestamp".to_string()),
        });
    }

    if let Some(json) = json_metadata {
        // 2. Chapters from yt-dlp
        let chapters = parse_chapters(json);

        // 3. Description timestamps (only if no chapters)
        if chapters.is_empty() {
            if let Some(description) = json.get("description").and_then(|v| v.as_str()) {
                let desc_timestamps = parse_description_timestamps(description);
                timestamps.extend(desc_timestamps);
            }
        } else {
            timestamps.extend(chapters);
        }
    }

    // Sort by time
    timestamps.sort_by_key(|t| t.time_seconds);

    // Deduplicate nearby timestamps
    deduplicate_timestamps(&mut timestamps);

    timestamps
}

/// Remove timestamps that are too close to each other
///
/// If two timestamps are within 3 seconds, keep the one with a label
/// (or the first one if both have labels).
fn deduplicate_timestamps(timestamps: &mut Vec<VideoTimestamp>) {
    if timestamps.len() < 2 {
        return;
    }

    let mut i = 0;
    while i < timestamps.len() - 1 {
        let diff = (timestamps[i + 1].time_seconds - timestamps[i].time_seconds).abs();
        if diff <= 3 {
            // Keep the one with a label, or prefer non-Description source
            let keep_second = timestamps[i + 1].label.is_some() && timestamps[i].label.is_none()
                || (timestamps[i].source == TimestampSource::Description
                    && timestamps[i + 1].source != TimestampSource::Description);

            if keep_second {
                timestamps.remove(i);
            } else {
                timestamps.remove(i + 1);
            }
        } else {
            i += 1;
        }
    }
}

/// Select the best timestamps for UI display
///
/// Returns up to `max_count` timestamps, evenly distributed across the video.
/// URL timestamps are always included if present.
pub fn select_best_timestamps(timestamps: &[VideoTimestamp], max_count: usize) -> Vec<&VideoTimestamp> {
    if timestamps.len() <= max_count {
        return timestamps.iter().collect();
    }

    let mut selected: Vec<&VideoTimestamp> = Vec::new();

    // Always include URL timestamp if present
    if let Some(url_ts) = timestamps.iter().find(|t| t.source == TimestampSource::Url) {
        selected.push(url_ts);
    }

    // Get remaining timestamps (excluding URL)
    let other_timestamps: Vec<_> = timestamps.iter().filter(|t| t.source != TimestampSource::Url).collect();

    let remaining_slots = max_count - selected.len();
    if other_timestamps.len() <= remaining_slots {
        selected.extend(other_timestamps);
    } else {
        // Sample evenly distributed timestamps
        let step = other_timestamps.len() as f64 / remaining_slots as f64;
        for i in 0..remaining_slots {
            let idx = (i as f64 * step) as usize;
            if idx < other_timestamps.len() {
                selected.push(other_timestamps[idx]);
            }
        }
    }

    // Sort by time again
    selected.sort_by_key(|t| t.time_seconds);
    selected
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract_url_timestamp() {
        let url = Url::parse("https://youtube.com/watch?v=abc&t=90").unwrap();
        let timestamps = extract_all_timestamps(&url, None);

        assert_eq!(timestamps.len(), 1);
        assert_eq!(timestamps[0].time_seconds, 90);
        assert_eq!(timestamps[0].source, TimestampSource::Url);
    }

    #[test]
    fn test_extract_chapters() {
        let url = Url::parse("https://youtube.com/watch?v=abc").unwrap();
        let json = json!({
            "chapters": [
                {"start_time": 0.0, "end_time": 60.0, "title": "Intro"},
                {"start_time": 60.0, "end_time": 120.0, "title": "Main"}
            ]
        });

        let timestamps = extract_all_timestamps(&url, Some(&json));
        assert_eq!(timestamps.len(), 2);
        assert_eq!(timestamps[0].source, TimestampSource::Chapter);
    }

    #[test]
    fn test_description_fallback() {
        let url = Url::parse("https://youtube.com/watch?v=abc").unwrap();
        let json = json!({
            "description": "0:00 Intro\n1:30 Main"
        });

        let timestamps = extract_all_timestamps(&url, Some(&json));
        assert_eq!(timestamps.len(), 2);
        assert_eq!(timestamps[0].source, TimestampSource::Description);
    }

    #[test]
    fn test_chapters_override_description() {
        let url = Url::parse("https://youtube.com/watch?v=abc").unwrap();
        let json = json!({
            "chapters": [
                {"start_time": 0.0, "end_time": 60.0, "title": "Chapter"}
            ],
            "description": "0:00 Intro\n1:30 Main"
        });

        let timestamps = extract_all_timestamps(&url, Some(&json));
        assert_eq!(timestamps.len(), 1);
        assert_eq!(timestamps[0].source, TimestampSource::Chapter);
    }

    #[test]
    fn test_url_plus_chapters() {
        let url = Url::parse("https://youtube.com/watch?v=abc&t=90").unwrap();
        let json = json!({
            "chapters": [
                {"start_time": 0.0, "end_time": 60.0, "title": "Intro"}
            ]
        });

        let timestamps = extract_all_timestamps(&url, Some(&json));
        assert_eq!(timestamps.len(), 2);
        // Should be sorted: chapter at 0, URL at 90
        assert_eq!(timestamps[0].time_seconds, 0);
        assert_eq!(timestamps[1].time_seconds, 90);
    }

    #[test]
    fn test_deduplication() {
        let url = Url::parse("https://youtube.com/watch?v=abc&t=0").unwrap();
        let json = json!({
            "chapters": [
                {"start_time": 0.0, "end_time": 60.0, "title": "Intro"},
                {"start_time": 2.0, "end_time": 120.0, "title": "Almost same"}
            ]
        });

        let timestamps = extract_all_timestamps(&url, Some(&json));
        // URL at 0 and chapter at 0 should be deduplicated
        // Chapter at 2 is within 3 seconds of chapter at 0
        assert!(timestamps.len() <= 2);
    }

    #[test]
    fn test_select_best_timestamps() {
        let timestamps = vec![
            VideoTimestamp {
                source: TimestampSource::Chapter,
                time_seconds: 0,
                end_seconds: None,
                label: Some("A".to_string()),
            },
            VideoTimestamp {
                source: TimestampSource::Chapter,
                time_seconds: 60,
                end_seconds: None,
                label: Some("B".to_string()),
            },
            VideoTimestamp {
                source: TimestampSource::Chapter,
                time_seconds: 120,
                end_seconds: None,
                label: Some("C".to_string()),
            },
            VideoTimestamp {
                source: TimestampSource::Chapter,
                time_seconds: 180,
                end_seconds: None,
                label: Some("D".to_string()),
            },
            VideoTimestamp {
                source: TimestampSource::Chapter,
                time_seconds: 240,
                end_seconds: None,
                label: Some("E".to_string()),
            },
        ];

        let selected = select_best_timestamps(&timestamps, 3);
        assert_eq!(selected.len(), 3);
        // Should be evenly distributed
    }
}
