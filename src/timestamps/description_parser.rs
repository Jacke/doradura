//! Video description timestamp parser
//!
//! Extracts timestamps from video descriptions.
//! Common formats in YouTube descriptions:
//! - `0:00 Intro`
//! - `1:23 Verse 1`
//! - `12:34 Chorus`
//! - `1:23:45 Extended section`
//! - `[0:00] Introduction`
//! - `(1:30) Main part`

use super::{TimestampSource, VideoTimestamp};
use once_cell::sync::Lazy;
use regex::Regex;

/// Regex for parsing timestamp lines in descriptions
/// Matches: "0:00 Text", "1:23 Text", "12:34:56 Text"
/// Also: "[0:00] Text", "(1:23) Text"
static TIMESTAMP_LINE_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[\[\(\s]*(\d{1,2}):(\d{2})(?::(\d{2}))?[\]\)\s]*[-–—:]?\s*(.+)$").unwrap());

/// Alternative regex for timestamps at end of line
/// Matches: "Intro - 0:00", "Chapter 1 (1:23)"
static TIMESTAMP_END_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(.+?)[\s\-–—]+[\[\(]?(\d{1,2}):(\d{2})(?::(\d{2}))?[\]\)]?\s*$").unwrap());

/// Parse timestamps from video description text
///
/// Looks for lines containing timestamps in formats:
/// - `0:00 Label` (timestamp at start)
/// - `Label - 0:00` (timestamp at end)
///
/// # Arguments
///
/// * `description` - The video description text
///
/// # Returns
///
/// A vector of `VideoTimestamp` entries found in the description
pub fn parse_description_timestamps(description: &str) -> Vec<VideoTimestamp> {
    let mut timestamps = Vec::new();

    for line in description.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Try timestamp at start of line
        if let Some(ts) = parse_timestamp_at_start(trimmed) {
            timestamps.push(ts);
            continue;
        }

        // Try timestamp at end of line
        if let Some(ts) = parse_timestamp_at_end(trimmed) {
            timestamps.push(ts);
        }
    }

    timestamps
}

fn parse_timestamp_at_start(line: &str) -> Option<VideoTimestamp> {
    let caps = TIMESTAMP_LINE_REGEX.captures(line)?;

    let part1: i64 = caps.get(1)?.as_str().parse().ok()?;
    let part2: i64 = caps.get(2)?.as_str().parse().ok()?;
    let part3: Option<i64> = caps.get(3).and_then(|m| m.as_str().parse().ok());
    let label = caps.get(4).map(|m| m.as_str().trim().to_string());

    let time_seconds = if let Some(seconds) = part3 {
        // HH:MM:SS format
        part1 * 3600 + part2 * 60 + seconds
    } else {
        // MM:SS format
        part1 * 60 + part2
    };

    // Skip if label is empty or just punctuation
    let label = label.filter(|l| l.chars().any(|c| c.is_alphanumeric()));

    Some(VideoTimestamp {
        source: TimestampSource::Description,
        time_seconds,
        end_seconds: None,
        label,
    })
}

fn parse_timestamp_at_end(line: &str) -> Option<VideoTimestamp> {
    let caps = TIMESTAMP_END_REGEX.captures(line)?;

    let label = caps.get(1).map(|m| m.as_str().trim().to_string());
    let part1: i64 = caps.get(2)?.as_str().parse().ok()?;
    let part2: i64 = caps.get(3)?.as_str().parse().ok()?;
    let part3: Option<i64> = caps.get(4).and_then(|m| m.as_str().parse().ok());

    let time_seconds = if let Some(seconds) = part3 {
        part1 * 3600 + part2 * 60 + seconds
    } else {
        part1 * 60 + part2
    };

    // Skip if label is empty or just punctuation
    let label = label.filter(|l| l.chars().any(|c| c.is_alphanumeric()));

    Some(VideoTimestamp {
        source: TimestampSource::Description,
        time_seconds,
        end_seconds: None,
        label,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic_timestamps() {
        let desc = "0:00 Intro\n1:23 Verse 1\n2:45 Chorus";
        let timestamps = parse_description_timestamps(desc);

        assert_eq!(timestamps.len(), 3);
        assert_eq!(timestamps[0].time_seconds, 0);
        assert_eq!(timestamps[0].label, Some("Intro".to_string()));
        assert_eq!(timestamps[1].time_seconds, 83); // 1:23 = 83 seconds
        assert_eq!(timestamps[2].time_seconds, 165); // 2:45 = 165 seconds
    }

    #[test]
    fn test_parse_with_hours() {
        let desc = "1:23:45 Long chapter";
        let timestamps = parse_description_timestamps(desc);

        assert_eq!(timestamps.len(), 1);
        assert_eq!(timestamps[0].time_seconds, 5025); // 1*3600 + 23*60 + 45
    }

    #[test]
    fn test_parse_with_brackets() {
        let desc = "[0:00] Introduction\n(1:30) Main content\n[2:00] - Conclusion";
        let timestamps = parse_description_timestamps(desc);

        assert_eq!(timestamps.len(), 3);
        assert_eq!(timestamps[0].time_seconds, 0);
        assert_eq!(timestamps[1].time_seconds, 90);
        assert_eq!(timestamps[2].time_seconds, 120);
    }

    #[test]
    fn test_parse_timestamp_at_end() {
        let desc = "Introduction - 0:00\nMain Part (1:30)\nConclusion – 3:00";
        let timestamps = parse_description_timestamps(desc);

        assert_eq!(timestamps.len(), 3);
        assert_eq!(timestamps[0].label, Some("Introduction".to_string()));
        assert_eq!(timestamps[0].time_seconds, 0);
        assert_eq!(timestamps[1].label, Some("Main Part".to_string()));
        assert_eq!(timestamps[1].time_seconds, 90);
    }

    #[test]
    fn test_skip_non_timestamp_lines() {
        let desc = "This is a great video!\n\n0:00 Start\nSubscribe for more!";
        let timestamps = parse_description_timestamps(desc);

        assert_eq!(timestamps.len(), 1);
        assert_eq!(timestamps[0].time_seconds, 0);
    }

    #[test]
    fn test_empty_description() {
        let timestamps = parse_description_timestamps("");
        assert!(timestamps.is_empty());
    }

    #[test]
    fn test_description_without_timestamps() {
        let desc = "This video is about programming.\nNo timestamps here.";
        let timestamps = parse_description_timestamps(desc);
        assert!(timestamps.is_empty());
    }
}
