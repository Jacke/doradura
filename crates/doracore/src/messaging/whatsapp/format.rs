//! Text-formatting helpers for the WhatsApp adapter.
//!
//! WhatsApp renders its own lightweight markup (`*bold*`, `_italic_`,
//! `~strike~`, ```` ```mono``` ````) and does **not** understand HTML. Core
//! flows author bodies in [`TextStyle::Html`] (the Telegram-native style), so
//! this module converts those to WhatsApp markup; [`TextStyle::Plain`] and
//! [`TextStyle::Markdown`] pass through unchanged. It also holds the
//! field-length clamps the Cloud API enforces on interactive elements.

use crate::messaging::types::TextStyle;
use lazy_regex::regex;

/// WhatsApp interactive field length ceilings (Cloud API rejects longer).
pub const BUTTON_TITLE_MAX: usize = 20;
pub const LIST_ROW_TITLE_MAX: usize = 24;
pub const LIST_ROW_DESC_MAX: usize = 72;
pub const LIST_BUTTON_LABEL_MAX: usize = 20;
pub const SECTION_TITLE_MAX: usize = 24;
/// Body text ceiling for interactive messages (4096 for plain text bodies).
pub const INTERACTIVE_BODY_MAX: usize = 1024;

/// Render a body for WhatsApp given the author's intended style.
pub fn to_whatsapp_text(body: &str, style: TextStyle) -> String {
    match style {
        TextStyle::Plain | TextStyle::Markdown => body.to_string(),
        TextStyle::Html => html_to_whatsapp(body),
    }
}

/// Best-effort HTML → WhatsApp-markup conversion.
///
/// Handles the small tag vocabulary the bot actually emits (`<b>`/`<strong>`,
/// `<i>`/`<em>`, `<code>`/`<pre>`, `<a href>`, `<br>`), strips everything else,
/// and unescapes HTML entities **last** so literal `&lt;` in the source never
/// turns into a parsed tag.
fn html_to_whatsapp(html: &str) -> String {
    let mut s = html.to_string();

    // Links: <a href="URL">TEXT</a> → TEXT (URL). Done before generic strip.
    let link = regex!(r#"(?is)<a\s+[^>]*href\s*=\s*["']([^"']*)["'][^>]*>(.*?)</a>"#);
    s = link
        .replace_all(&s, |c: &regex::Captures| {
            let url = c.get(1).map_or("", |m| m.as_str());
            let text = c.get(2).map_or("", |m| m.as_str());
            if text.trim().is_empty() || text.trim() == url.trim() {
                url.to_string()
            } else {
                format!("{text} ({url})")
            }
        })
        .into_owned();

    // Bold / italic / monospace.
    s = regex!(r"(?is)</?(?:b|strong)>").replace_all(&s, "*").into_owned();
    s = regex!(r"(?is)</?(?:i|em)>").replace_all(&s, "_").into_owned();
    s = regex!(r"(?is)</?(?:code|pre|tt)>").replace_all(&s, "```").into_owned();

    // Line breaks.
    s = regex!(r"(?is)<br\s*/?>").replace_all(&s, "\n").into_owned();

    // Drop any remaining tags.
    s = regex!(r"(?is)<[^>]+>").replace_all(&s, "").into_owned();

    unescape_entities(&s)
}

/// Unescape the HTML entities the bot's escaper produces.
fn unescape_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

/// Truncate to at most `max` characters (not bytes), appending `…` when cut, so
/// the result still fits `max`. Counts by `char` to stay UTF-8 safe.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let keep = max.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_and_markdown_pass_through() {
        assert_eq!(to_whatsapp_text("*hi* _there_", TextStyle::Plain), "*hi* _there_");
        assert_eq!(to_whatsapp_text("*hi*", TextStyle::Markdown), "*hi*");
    }

    #[test]
    fn html_bold_italic_code() {
        assert_eq!(
            to_whatsapp_text("<b>Bold</b> and <i>it</i> and <code>x</code>", TextStyle::Html),
            "*Bold* and _it_ and ```x```"
        );
    }

    #[test]
    fn html_link_becomes_text_and_url() {
        assert_eq!(
            to_whatsapp_text(r#"<a href="https://e.com">Click</a>"#, TextStyle::Html),
            "Click (https://e.com)"
        );
    }

    #[test]
    fn html_link_text_equal_to_url_collapses() {
        assert_eq!(
            to_whatsapp_text(r#"<a href="https://e.com">https://e.com</a>"#, TextStyle::Html),
            "https://e.com"
        );
    }

    #[test]
    fn html_br_and_entities() {
        assert_eq!(
            to_whatsapp_text("a&amp;b<br>c &lt;d&gt;", TextStyle::Html),
            "a&b\nc <d>"
        );
    }

    #[test]
    fn html_strips_unknown_tags() {
        assert_eq!(to_whatsapp_text("<span class='x'>hi</span>", TextStyle::Html), "hi");
    }

    #[test]
    fn truncate_keeps_short_strings() {
        assert_eq!(truncate("hello", 20), "hello");
    }

    #[test]
    fn truncate_cuts_long_strings_with_ellipsis() {
        let out = truncate("abcdefghij", 5);
        assert_eq!(out, "abcd…");
        assert_eq!(out.chars().count(), 5);
    }

    #[test]
    fn truncate_is_utf8_safe() {
        // Multibyte chars must not be split mid-byte.
        let out = truncate("абвгде", 3);
        assert_eq!(out.chars().count(), 3);
        assert_eq!(out, "аб…");
    }
}
