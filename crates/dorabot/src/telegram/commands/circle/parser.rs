//! Parsers for circle/cut command syntax.
//!
//! Pure-text → typed-segment conversion: `CutSegment`, time-range / timestamp /
//! command-shorthand (`full`, `first30`, `last15`, `middle20`) parsers, plus the
//! shared `format_timestamp` helper. Self-contained — no I/O, no Telegram /
//! ffmpeg deps. Extracted from `circle.rs` (was 2572 LOC) for clarity and
//! separate testability.

use itertools::Itertools;

/// Segment of video to cut
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct CutSegment {
    pub start_secs: i64,
    pub end_secs: i64,
}

pub fn parse_command_segment(text: &str, video_duration: Option<i64>) -> Option<(i64, i64, String)> {
    let normalized = text.trim().to_lowercase();

    // Strip speed modifiers if present (e.g., "first30 2x", "full speed1.5")
    // We'll just parse the segment here, speed will be handled separately
    let segment_part = normalized.split_whitespace().next().unwrap_or(&normalized);

    // full - entire video
    if segment_part == "full" {
        let duration = video_duration?;
        let end = duration.min(60); // Max 60 seconds for video notes
        return Some((0, end, format!("00:00-{}", format_timestamp(end))));
    }

    // first<N> - first N seconds (first30, first15, etc.)
    if let Some(num_str) = segment_part.strip_prefix("first")
        && let Ok(secs) = num_str.parse::<i64>()
        && secs > 0
        && secs <= 60
    {
        return Some((0, secs, format!("00:00-{}", format_timestamp(secs))));
    }

    // last<N> - last N seconds (last30, last15, etc.)
    if let Some(num_str) = segment_part.strip_prefix("last")
        && let Ok(secs) = num_str.parse::<i64>()
    {
        let duration = video_duration?;
        if secs > 0 && secs <= 60 && secs <= duration {
            let start = (duration - secs).max(0);
            return Some((
                start,
                duration,
                format!("{}-{}", format_timestamp(start), format_timestamp(duration)),
            ));
        }
    }

    // middle<N> - N seconds from the middle (middle30, middle15, etc.)
    if let Some(num_str) = segment_part.strip_prefix("middle")
        && let Ok(secs) = num_str.parse::<i64>()
    {
        let duration = video_duration?;
        if secs > 0 && secs <= 60 && secs <= duration {
            let start = ((duration - secs) / 2).max(0);
            let end = start + secs;
            return Some((
                start,
                end,
                format!("{}-{}", format_timestamp(start), format_timestamp(end)),
            ));
        }
    }

    None
}

/// Parse time range from text following a URL.
/// Accepts "HH:MM:SS-HH:MM:SS" or "MM:SS-MM:SS" after the URL.
pub fn parse_download_time_range(text: &str, url_text: &str) -> Option<(String, String, Option<f32>)> {
    let after = text.split(url_text).nth(1)?.trim();
    let mut parts = after.split_whitespace();
    let range_text = parts.next()?;
    if range_text.is_empty() {
        return None;
    }
    let normalized = range_text.replace(['—', '–', '−'], "-");
    let (start_str, end_str) = normalized.split_once('-')?;
    let start_secs = parse_timestamp_secs(start_str)?;
    let end_secs = parse_timestamp_secs(end_str)?;
    if end_secs <= start_secs {
        return None;
    }
    // Check remaining text for speed modifier (e.g., "2x", "1.5x", "speed2")
    let remaining: String = parts.join(" ");
    let speed = if remaining.is_empty() {
        None
    } else {
        parse_speed_modifier(&remaining)
    };
    Some((start_str.to_string(), end_str.to_string(), speed))
}

pub fn parse_time_range_secs(text: &str) -> Option<(i64, i64)> {
    let normalized = text.trim().replace(['—', '–', '−'], "-");
    // Strip trailing speed modifier (e.g., "2:40:53-2:42:19 2x" -> "2:40:53-2:42:19")
    let timestamp_part = normalized
        .rsplit_once(' ')
        .and_then(|(before, after)| {
            let lower = after.to_lowercase();
            if lower.ends_with('x') || lower.starts_with('x') || lower.starts_with("speed") {
                Some(before)
            } else {
                None
            }
        })
        .unwrap_or(&normalized);
    let cleaned = timestamp_part.replace(' ', "");
    let (start_str, end_str) = cleaned.split_once('-')?;
    let start = parse_timestamp_secs(start_str)?;
    let end = parse_timestamp_secs(end_str)?;
    if end <= start {
        return None;
    }
    Some((start, end))
}

pub fn parse_timestamp_secs(text: &str) -> Option<i64> {
    let parts: Vec<&str> = text.split(':').collect();
    match parts.len() {
        2 => {
            let minutes: i64 = parts[0].parse().ok()?;
            let seconds: i64 = parts[1].parse().ok()?;
            if minutes < 0 || !(0..60).contains(&seconds) {
                return None;
            }
            Some(minutes * 60 + seconds)
        }
        3 => {
            let hours: i64 = parts[0].parse().ok()?;
            let minutes: i64 = parts[1].parse().ok()?;
            let seconds: i64 = parts[2].parse().ok()?;
            if hours < 0 || minutes < 0 || !(0..60).contains(&minutes) || !(0..60).contains(&seconds) {
                return None;
            }
            Some(hours * 3600 + minutes * 60 + seconds)
        }
        _ => None,
    }
}

pub fn format_timestamp(secs: i64) -> String {
    let secs = secs.max(0);
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

pub fn parse_segments_spec(text: &str, video_duration: Option<i64>) -> Option<(Vec<CutSegment>, String, Option<f32>)> {
    let normalized = text.trim().replace(['—', '–', '−'], "-");

    // Extract speed modifier from anywhere in the text (e.g., "first30 2x", "1.5x full", "speed2 middle30")
    let speed = parse_speed_modifier(&normalized);

    let raw_parts: Vec<&str> = normalized
        .split([',', ';', '\n'])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if raw_parts.is_empty() {
        return None;
    }

    let mut segments = Vec::new();
    let mut pretty_parts = Vec::new();
    for part in raw_parts {
        // Try parsing as command first (full, first30, last30, etc.)
        if let Some((start_secs, end_secs, pretty)) = parse_command_segment(part, video_duration) {
            segments.push(CutSegment { start_secs, end_secs });
            pretty_parts.push(pretty);
        } else if let Some((start_secs, end_secs)) = parse_time_range_secs(part) {
            // Fall back to time range parsing
            segments.push(CutSegment { start_secs, end_secs });
            pretty_parts.push(format!(
                "{}-{}",
                format_timestamp(start_secs),
                format_timestamp(end_secs)
            ));
        } else {
            return None; // Invalid format
        }
    }

    Some((segments, pretty_parts.join(", "), speed))
}

pub fn parse_audio_segments_spec(text: &str, audio_duration: Option<i64>) -> Option<(Vec<CutSegment>, String)> {
    let normalized = text.trim();
    if normalized.is_empty() {
        return None;
    }

    let raw_parts: Vec<&str> = normalized
        .split([',', ';', '\n'])
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();
    if raw_parts.is_empty() {
        return None;
    }

    let mut segments = Vec::new();
    let mut pretty_parts = Vec::new();
    for part in raw_parts {
        if let Some((start_secs, end_secs, pretty)) = parse_audio_command_segment(part, audio_duration) {
            segments.push(CutSegment { start_secs, end_secs });
            pretty_parts.push(pretty);
        } else if let Some((start_secs, end_secs)) = parse_time_range_secs(part) {
            segments.push(CutSegment { start_secs, end_secs });
            pretty_parts.push(format!(
                "{}-{}",
                format_timestamp(start_secs),
                format_timestamp(end_secs)
            ));
        } else {
            return None;
        }
    }

    Some((segments, pretty_parts.join(", ")))
}

pub fn parse_speed_modifier(text: &str) -> Option<f32> {
    let lower = text.to_lowercase();

    // Look for patterns like: "2x", "1.5x", "speed2", "speed1.5", "x2", "x1.5"
    for word in lower.split_whitespace() {
        // "2x", "1.5x"
        if let Some(num_str) = word.strip_suffix('x')
            && let Ok(speed) = num_str.parse::<f32>()
            && speed > 0.0
            && speed <= 2.0
        {
            return Some(speed);
        }
        // "x2", "x1.5"
        if let Some(num_str) = word.strip_prefix('x')
            && let Ok(speed) = num_str.parse::<f32>()
            && speed > 0.0
            && speed <= 2.0
        {
            return Some(speed);
        }
        // "speed2", "speed1.5"
        if let Some(num_str) = word.strip_prefix("speed")
            && let Ok(speed) = num_str.parse::<f32>()
            && speed > 0.0
            && speed <= 2.0
        {
            return Some(speed);
        }
    }

    None
}

pub(super) fn parse_audio_command_segment(text: &str, audio_duration: Option<i64>) -> Option<(i64, i64, String)> {
    let normalized = text.trim().to_lowercase();
    let segment_part = normalized.split_whitespace().next().unwrap_or(&normalized);
    let duration = audio_duration?;

    if segment_part == "full" {
        return Some((0, duration, format!("00:00-{}", format_timestamp(duration))));
    }

    if let Some(num_str) = segment_part.strip_prefix("first")
        && let Ok(secs) = num_str.parse::<i64>()
        && secs > 0
    {
        let end = secs.min(duration);
        return Some((0, end, format!("00:00-{}", format_timestamp(end))));
    }

    if let Some(num_str) = segment_part.strip_prefix("last")
        && let Ok(secs) = num_str.parse::<i64>()
        && secs > 0
        && secs <= duration
    {
        let start = (duration - secs).max(0);
        return Some((
            start,
            duration,
            format!("{}-{}", format_timestamp(start), format_timestamp(duration)),
        ));
    }

    if let Some(num_str) = segment_part.strip_prefix("middle")
        && let Ok(secs) = num_str.parse::<i64>()
        && secs > 0
        && secs <= duration
    {
        let start = ((duration - secs) / 2).max(0);
        let end = start + secs;
        return Some((
            start,
            end,
            format!("{}-{}", format_timestamp(start), format_timestamp(end)),
        ));
    }

    None
}
