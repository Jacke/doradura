//! YouTube chapters parser
//!
//! Extracts chapter information from yt-dlp JSON metadata.
//! YouTube chapters are returned in the `chapters` array with
//! `start_time`, `end_time`, and `title` fields.

use super::{TimestampSource, VideoTimestamp};
use serde_json::Value;

/// Parse chapters from yt-dlp JSON metadata
///
/// The chapters array format from yt-dlp:
/// ```json
/// {
///   "chapters": [
///     {"start_time": 0.0, "end_time": 60.0, "title": "Intro"},
///     {"start_time": 60.0, "end_time": 180.0, "title": "Main"}
///   ]
/// }
/// ```
///
/// # Arguments
///
/// * `json` - The full yt-dlp JSON metadata
///
/// # Returns
///
/// A vector of `VideoTimestamp` entries for each chapter
pub fn parse_chapters(json: &Value) -> Vec<VideoTimestamp> {
    let mut timestamps = Vec::new();

    if let Some(chapters) = json.get("chapters").and_then(|v| v.as_array()) {
        for chapter in chapters {
            let start_time = chapter
                .get("start_time")
                .and_then(|v| v.as_f64())
                .map(|t| t.round() as i64);

            let end_time = chapter
                .get("end_time")
                .and_then(|v| v.as_f64())
                .map(|t| t.round() as i64);

            let title = chapter.get("title").and_then(|v| v.as_str()).map(|s| s.to_string());

            if let Some(start) = start_time {
                timestamps.push(VideoTimestamp {
                    source: TimestampSource::Chapter,
                    time_seconds: start,
                    end_seconds: end_time,
                    label: title,
                });
            }
        }
    }

    timestamps
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_chapters_basic() {
        let json = json!({
            "chapters": [
                {"start_time": 0.0, "end_time": 60.0, "title": "Intro"},
                {"start_time": 60.0, "end_time": 180.0, "title": "Main Part"},
                {"start_time": 180.0, "end_time": 240.0, "title": "Outro"}
            ]
        });

        let chapters = parse_chapters(&json);
        assert_eq!(chapters.len(), 3);

        assert_eq!(chapters[0].time_seconds, 0);
        assert_eq!(chapters[0].end_seconds, Some(60));
        assert_eq!(chapters[0].label, Some("Intro".to_string()));

        assert_eq!(chapters[1].time_seconds, 60);
        assert_eq!(chapters[1].end_seconds, Some(180));
        assert_eq!(chapters[1].label, Some("Main Part".to_string()));

        assert_eq!(chapters[2].time_seconds, 180);
        assert_eq!(chapters[2].end_seconds, Some(240));
    }

    #[test]
    fn test_parse_chapters_empty() {
        let json = json!({"chapters": []});
        let chapters = parse_chapters(&json);
        assert!(chapters.is_empty());
    }

    #[test]
    fn test_parse_chapters_missing() {
        let json = json!({"title": "Some Video"});
        let chapters = parse_chapters(&json);
        assert!(chapters.is_empty());
    }

    #[test]
    fn test_parse_chapters_with_fractional_seconds() {
        let json = json!({
            "chapters": [
                {"start_time": 0.5, "end_time": 60.7, "title": "Chapter 1"}
            ]
        });

        let chapters = parse_chapters(&json);
        assert_eq!(chapters.len(), 1);
        assert_eq!(chapters[0].time_seconds, 1); // Rounded
        assert_eq!(chapters[0].end_seconds, Some(61)); // Rounded
    }

    #[test]
    fn test_parse_chapters_missing_title() {
        let json = json!({
            "chapters": [
                {"start_time": 0.0, "end_time": 60.0}
            ]
        });

        let chapters = parse_chapters(&json);
        assert_eq!(chapters.len(), 1);
        assert!(chapters[0].label.is_none());
    }
}
