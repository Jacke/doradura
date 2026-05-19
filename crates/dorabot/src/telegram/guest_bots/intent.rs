//! Parse "what does the caller want" from the mention + replied-to message.
//!
//! Two pieces of input:
//!   - `mention_text` — the message containing `@bot` (e.g. "@doradura_bot mp3")
//!   - `reply_text` — text of the message being replied to, if any
//!
//! URL precedence: first URL in reply_text (the common share case) wins;
//! falling back to a URL in mention_text. This matches the natural flow
//! "[Маша] https://yt.be/x" → "[Петя на реплае] @bot mp3".

use lazy_regex::{Lazy, Regex, lazy_regex};

/// Cached URL regex — same source as `telegram::commands::URL_REGEX` to
/// keep extraction behaviour consistent across the bot.
static URL_REGEX: Lazy<Regex> = lazy_regex!(r"https?://[^\s]+");

/// What format the caller asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestFormat {
    /// `@bot mp3` — audio
    Mp3,
    /// `@bot mp4` / default for videos with a URL
    Mp4,
    /// `@bot` with no further argument — caller wants the choose-format card
    Auto,
}

impl GuestFormat {
    fn parse(token: &str) -> Option<Self> {
        match token.trim().to_lowercase().as_str() {
            "mp3" | "audio" | "music" => Some(Self::Mp3),
            "mp4" | "video" => Some(Self::Mp4),
            _ => None,
        }
    }

    /// Cache key for popular_files / download_history lookups.
    pub fn db_key(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Mp4 | Self::Auto => "mp4",
        }
    }
}

/// Successfully parsed guest-bot request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedIntent {
    pub url: String,
    pub format: GuestFormat,
}

/// Parse the mention + reply texts. Returns `None` if no URL is reachable
/// anywhere — caller renders a help message instead.
pub fn parse(mention_text: &str, reply_text: &str) -> Option<ParsedIntent> {
    let url = extract_url(reply_text).or_else(|| extract_url(mention_text))?;

    // Token detection: scan everything AFTER the @mention (or anywhere if no
    // @ in mention_text) for the first known format word. URL itself is skipped.
    let after_mention = match mention_text.find('@') {
        Some(idx) => &mention_text[idx..],
        None => mention_text,
    };
    let format = after_mention
        .split_whitespace()
        .filter(|tok| !tok.starts_with("http") && !tok.starts_with('@'))
        .find_map(GuestFormat::parse)
        .unwrap_or(GuestFormat::Auto);

    Some(ParsedIntent { url, format })
}

fn extract_url(text: &str) -> Option<String> {
    URL_REGEX
        .find(text)
        .map(|m| m.as_str().trim_end_matches(['.', ',', ')', ']']).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_url_from_reply_with_explicit_mp3() {
        let i = parse("@doradura_bot mp3", "https://youtu.be/abc").unwrap();
        assert_eq!(i.url, "https://youtu.be/abc");
        assert_eq!(i.format, GuestFormat::Mp3);
    }

    #[test]
    fn parse_url_from_mention_when_no_reply() {
        let i = parse("@doradura_bot mp4 https://youtu.be/xyz", "").unwrap();
        assert_eq!(i.url, "https://youtu.be/xyz");
        assert_eq!(i.format, GuestFormat::Mp4);
    }

    #[test]
    fn parse_without_format_token_yields_auto() {
        let i = parse("@doradura_bot", "https://youtu.be/abc").unwrap();
        assert_eq!(i.format, GuestFormat::Auto);
    }

    #[test]
    fn parse_returns_none_when_no_url_anywhere() {
        assert!(parse("@doradura_bot hello", "what's up").is_none());
    }

    #[test]
    fn parse_strips_trailing_punctuation_from_url() {
        let i = parse("@bot", "see https://yt.be/x.").unwrap();
        assert_eq!(i.url, "https://yt.be/x");
    }

    #[test]
    fn parse_picks_reply_url_over_mention_url() {
        let i = parse("@bot mp3 https://wrong.example", "https://right.example/track").unwrap();
        assert_eq!(i.url, "https://right.example/track");
    }

    #[test]
    fn format_aliases_recognized() {
        assert_eq!(GuestFormat::parse("audio"), Some(GuestFormat::Mp3));
        assert_eq!(GuestFormat::parse("VIDEO"), Some(GuestFormat::Mp4));
        assert_eq!(GuestFormat::parse("garbage"), None);
    }
}
