//! Video timestamp parsing and management
//!
//! This module extracts timestamps from various sources:
//! - URL parameters (e.g., `?t=123`, `&t=1m30s`)
//! - YouTube chapters from yt-dlp metadata
//! - Video descriptions (e.g., "0:00 Intro", "1:23 Chorus")

mod chapter_parser;
mod description_parser;
mod extractor;
mod url_parser;

pub use chapter_parser::parse_chapters;
pub use description_parser::parse_description_timestamps;
pub use extractor::{extract_all_timestamps, select_best_timestamps};
pub use url_parser::parse_url_timestamp;

use serde::{Deserialize, Serialize};

/// A timestamp marker in a video
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoTimestamp {
    /// Source of the timestamp
    pub source: TimestampSource,
    /// Start time in seconds
    pub time_seconds: i64,
    /// End time in seconds (only for chapters)
    pub end_seconds: Option<i64>,
    /// Label/title for this timestamp
    pub label: Option<String>,
}

/// Source of a video timestamp
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimestampSource {
    /// From URL parameter (?t=123 or &t=1m30s)
    Url,
    /// From yt-dlp chapters array
    Chapter,
    /// Parsed from video description
    Description,
}

impl TimestampSource {
    /// Convert to string for database storage
    pub fn as_str(&self) -> &'static str {
        match self {
            TimestampSource::Url => "url",
            TimestampSource::Chapter => "chapter",
            TimestampSource::Description => "description",
        }
    }

    /// Parse from database string
    pub fn parse(s: &str) -> Self {
        match s {
            "url" => TimestampSource::Url,
            "chapter" => TimestampSource::Chapter,
            "description" => TimestampSource::Description,
            _ => TimestampSource::Description,
        }
    }
}

impl VideoTimestamp {
    /// Format timestamp as MM:SS or HH:MM:SS string
    pub fn format_time(&self) -> String {
        format_timestamp(self.time_seconds)
    }

    /// Get display label (truncated if needed)
    pub fn display_label(&self, max_len: usize) -> String {
        match &self.label {
            Some(label) if label.chars().count() > max_len => {
                format!("{}...", label.chars().take(max_len - 3).collect::<String>())
            }
            Some(label) => label.clone(),
            None => match self.source {
                TimestampSource::Url => "URL".to_string(),
                TimestampSource::Chapter => "Chapter".to_string(),
                TimestampSource::Description => "Marker".to_string(),
            },
        }
    }
}

/// Parse "MM:SS" or "HH:MM:SS" string to seconds
pub fn parse_timestamp_to_secs(text: &str) -> Option<i64> {
    let parts: Vec<&str> = text.split(':').collect();
    match parts.len() {
        2 => {
            let minutes: i64 = parts[0].parse().ok()?;
            let seconds: i64 = parts[1].parse().ok()?;
            Some(minutes * 60 + seconds)
        }
        3 => {
            let hours: i64 = parts[0].parse().ok()?;
            let minutes: i64 = parts[1].parse().ok()?;
            let seconds: i64 = parts[2].parse().ok()?;
            Some(hours * 3600 + minutes * 60 + seconds)
        }
        _ => None,
    }
}

/// Filter timestamps to only those within a time range, adjusting to clip-relative times.
///
/// Timestamps outside [start_secs, end_secs) are removed.
/// Remaining timestamps have start_secs subtracted from their time_seconds.
pub fn filter_timestamps_for_range(
    timestamps: &[VideoTimestamp],
    start_secs: i64,
    end_secs: i64,
) -> Vec<VideoTimestamp> {
    timestamps
        .iter()
        .filter(|ts| ts.time_seconds >= start_secs && ts.time_seconds < end_secs)
        .map(|ts| VideoTimestamp {
            source: ts.source,
            time_seconds: ts.time_seconds - start_secs,
            end_seconds: ts.end_seconds.map(|e| (e - start_secs).min(end_secs - start_secs)),
            label: ts.label.clone(),
        })
        .collect()
}

/// Format seconds as MM:SS or HH:MM:SS
pub fn format_timestamp(seconds: i64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;

    if hours > 0 {
        format!("{}:{:02}:{:02}", hours, minutes, secs)
    } else {
        format!("{}:{:02}", minutes, secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_timestamp() {
        assert_eq!(format_timestamp(0), "0:00");
        assert_eq!(format_timestamp(65), "1:05");
        assert_eq!(format_timestamp(3661), "1:01:01");
        assert_eq!(format_timestamp(7200), "2:00:00");
    }

    #[test]
    fn test_timestamp_source_roundtrip() {
        assert_eq!(TimestampSource::parse("url"), TimestampSource::Url);
        assert_eq!(TimestampSource::parse("chapter"), TimestampSource::Chapter);
        assert_eq!(TimestampSource::parse("description"), TimestampSource::Description);
        assert_eq!(TimestampSource::Url.as_str(), "url");
    }

    #[test]
    fn test_display_label_truncation() {
        let ts = VideoTimestamp {
            source: TimestampSource::Chapter,
            time_seconds: 60,
            end_seconds: None,
            label: Some("This is a very long chapter title that should be truncated".to_string()),
        };
        let label = ts.display_label(15);
        assert!(label.len() <= 15);
        assert!(label.ends_with("..."));
    }

    #[test]
    fn test_parse_timestamp_to_secs() {
        assert_eq!(parse_timestamp_to_secs("0:10"), Some(10));
        assert_eq!(parse_timestamp_to_secs("1:30"), Some(90));
        assert_eq!(parse_timestamp_to_secs("1:00:00"), Some(3600));
        assert_eq!(parse_timestamp_to_secs("invalid"), None);
    }

    #[test]
    fn test_filter_timestamps_for_range() {
        let timestamps = vec![
            VideoTimestamp {
                source: TimestampSource::Chapter,
                time_seconds: 5,
                end_seconds: Some(10),
                label: Some("Before".into()),
            },
            VideoTimestamp {
                source: TimestampSource::Chapter,
                time_seconds: 15,
                end_seconds: Some(25),
                label: Some("Inside1".into()),
            },
            VideoTimestamp {
                source: TimestampSource::Chapter,
                time_seconds: 30,
                end_seconds: Some(40),
                label: Some("Inside2".into()),
            },
            VideoTimestamp {
                source: TimestampSource::Chapter,
                time_seconds: 50,
                end_seconds: None,
                label: Some("After".into()),
            },
        ];
        // Range: 10s - 40s
        let filtered = filter_timestamps_for_range(&timestamps, 10, 40);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].time_seconds, 5); // 15 - 10
        assert_eq!(filtered[0].label.as_deref(), Some("Inside1"));
        assert_eq!(filtered[1].time_seconds, 20); // 30 - 10
        assert_eq!(filtered[1].label.as_deref(), Some("Inside2"));
    }
}
