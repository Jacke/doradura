//! URL timestamp parameter parser
//!
//! Extracts timestamps from URL query parameters and fragments.
//! Supports YouTube-style formats:
//! - `?t=123` (seconds)
//! - `&t=1m30s` (minutes and seconds)
//! - `&t=1h2m30s` (hours, minutes, seconds)
//! - `#t=90` (fragment)

use once_cell::sync::Lazy;
use regex::Regex;
use url::Url;

/// Regex for parsing time values like "1h2m30s", "1m30s", "30s", "30"
static TIME_FORMAT_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(?:(\d+)h)?(?:(\d+)m)?(\d+)s?$").unwrap());

/// Extract timestamp from URL parameters
///
/// Handles various formats:
/// - `?t=123` - seconds as integer
/// - `&t=90` - seconds in query string
/// - `?t=1m30s` - time format
/// - `#t=90` - fragment identifier
///
/// # Examples
///
/// ```
/// use url::Url;
/// use doradura::timestamps::parse_url_timestamp;
///
/// let url = Url::parse("https://youtube.com/watch?v=abc&t=90").unwrap();
/// assert_eq!(parse_url_timestamp(&url), Some(90));
///
/// let url = Url::parse("https://youtube.com/watch?v=abc&t=1m30s").unwrap();
/// assert_eq!(parse_url_timestamp(&url), Some(90));
/// ```
pub fn parse_url_timestamp(url: &Url) -> Option<i64> {
    // Check query parameters for 't' or 'start'
    for (key, value) in url.query_pairs() {
        if key == "t" || key == "start" || key == "time_continue" {
            if let Some(secs) = parse_time_value(&value) {
                return Some(secs);
            }
        }
    }

    // Check fragment (hash) for t=
    if let Some(fragment) = url.fragment() {
        // Handle #t=90 format
        if let Some(value) = fragment.strip_prefix("t=") {
            if let Some(secs) = parse_time_value(value) {
                return Some(secs);
            }
        }
        // Handle #90 format (just seconds)
        if let Ok(secs) = fragment.parse::<i64>() {
            return Some(secs);
        }
    }

    None
}

/// Parse a time value string into seconds
///
/// Supports:
/// - Pure seconds: "123"
/// - Time format: "1h2m30s", "1m30s", "30s"
fn parse_time_value(value: &str) -> Option<i64> {
    let value = value.trim();

    // Try pure seconds first
    if let Ok(secs) = value.parse::<i64>() {
        return Some(secs);
    }

    // Try time format (1h2m30s, 1m30s, 30s)
    if let Some(caps) = TIME_FORMAT_REGEX.captures(value) {
        let hours: i64 = caps.get(1).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        let minutes: i64 = caps.get(2).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        let seconds: i64 = caps.get(3).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        return Some(hours * 3600 + minutes * 60 + seconds);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_seconds() {
        let url = Url::parse("https://youtube.com/watch?v=abc&t=90").unwrap();
        assert_eq!(parse_url_timestamp(&url), Some(90));
    }

    #[test]
    fn test_parse_time_format() {
        let url = Url::parse("https://youtube.com/watch?v=abc&t=1m30s").unwrap();
        assert_eq!(parse_url_timestamp(&url), Some(90));

        let url = Url::parse("https://youtube.com/watch?v=abc&t=1h2m30s").unwrap();
        assert_eq!(parse_url_timestamp(&url), Some(3750)); // 1*3600 + 2*60 + 30
    }

    #[test]
    fn test_parse_fragment() {
        let url = Url::parse("https://youtube.com/watch?v=abc#t=60").unwrap();
        assert_eq!(parse_url_timestamp(&url), Some(60));

        let url = Url::parse("https://youtube.com/watch?v=abc#90").unwrap();
        assert_eq!(parse_url_timestamp(&url), Some(90));
    }

    #[test]
    fn test_no_timestamp() {
        let url = Url::parse("https://youtube.com/watch?v=abc").unwrap();
        assert_eq!(parse_url_timestamp(&url), None);
    }

    #[test]
    fn test_start_parameter() {
        let url = Url::parse("https://youtube.com/watch?v=abc&start=120").unwrap();
        assert_eq!(parse_url_timestamp(&url), Some(120));
    }

    #[test]
    fn test_time_continue_parameter() {
        let url = Url::parse("https://youtube.com/watch?v=abc&time_continue=45").unwrap();
        assert_eq!(parse_url_timestamp(&url), Some(45));
    }

    #[test]
    fn test_parse_time_value_edge_cases() {
        assert_eq!(parse_time_value("0"), Some(0));
        assert_eq!(parse_time_value("30s"), Some(30));
        assert_eq!(parse_time_value("5m"), None); // Must have seconds
        assert_eq!(parse_time_value("5m0s"), Some(300));
    }
}
