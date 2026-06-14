//! Lyrics providers — an explicit registry over the lyrics sources, plus
//! **URL-based resolution** used by the manual-correction flow.
//!
//! The auto-match cascade lives in [`super::fetch_lyrics_smart`]; this module
//! adds the inverse path: given a *specific* lyrics URL the user supplied
//! (because the auto-match picked the wrong song), resolve it to a
//! [`LyricsResult`]. Each provider knows which URLs it owns and how to scrape
//! them. Reuses the parent module's HTML parser / section splitter.

use super::{LyricsResult, build_http_client, parse_genius_html, parse_sections};

/// Max lyrics page body — mirror the cap used for the auto-match Genius scrape.
const MAX_HTML_BYTES: usize = 5 * 1024 * 1024;

/// A lyrics source. Stable ids are persisted in `lyrics_overrides.provider`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LyricsProviderKind {
    Lrclib,
    Genius,
}

impl LyricsProviderKind {
    pub fn id(self) -> &'static str {
        match self {
            LyricsProviderKind::Lrclib => "lrclib",
            LyricsProviderKind::Genius => "genius",
        }
    }

    pub fn from_id(s: &str) -> Option<Self> {
        match s {
            "lrclib" => Some(LyricsProviderKind::Lrclib),
            "genius" => Some(LyricsProviderKind::Genius),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LyricsProviderKind::Lrclib => "LRCLIB",
            LyricsProviderKind::Genius => "Genius",
        }
    }
}

/// Which provider owns a user-supplied lyrics URL, if any.
pub fn provider_for_url(url: &str) -> Option<LyricsProviderKind> {
    let u = url.to_ascii_lowercase();
    if u.contains("genius.com") {
        Some(LyricsProviderKind::Genius)
    } else if u.contains("lrclib.net") {
        Some(LyricsProviderKind::Lrclib)
    } else {
        None
    }
}

/// Resolve a user-supplied lyrics URL to lyrics. Returns the provider it came
/// from plus the parsed result. `None` if the URL isn't a known provider, the
/// fetch failed, or no usable lyrics were found.
pub async fn fetch_from_url(url: &str) -> Option<(LyricsProviderKind, LyricsResult)> {
    match provider_for_url(url)? {
        LyricsProviderKind::Genius => fetch_genius_url(url).await.map(|r| (LyricsProviderKind::Genius, r)),
        LyricsProviderKind::Lrclib => fetch_lrclib_url(url).await.map(|r| (LyricsProviderKind::Lrclib, r)),
    }
}

/// Scrape a Genius song page → [`LyricsResult`]. Artist/title are parsed
/// best-effort from the page `<title>`.
async fn fetch_genius_url(url: &str) -> Option<LyricsResult> {
    let client = build_http_client()?;
    let resp = client
        .get(url)
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await
        .ok()?;
    if let Some(len) = resp.content_length()
        && len as usize > MAX_HTML_BYTES
    {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    if bytes.len() > MAX_HTML_BYTES {
        return None;
    }
    let html = String::from_utf8_lossy(&bytes).into_owned();
    let text = parse_genius_html(&html)?;
    let (sections, has_structure) = parse_sections(&text);
    if sections.is_empty() {
        return None;
    }
    let (artist, title) = parse_genius_page_title(&html);
    Some(LyricsResult {
        artist,
        artist_id: None,
        title,
        album: None,
        release_date: None,
        thumbnail_url: None,
        sections,
        has_structure,
    })
}

/// Fetch lyrics from an LRCLIB URL/id. Accepts the API form
/// (`lrclib.net/api/get/{id}`) and any URL whose last numeric path segment is
/// the track id.
async fn fetch_lrclib_url(url: &str) -> Option<LyricsResult> {
    let id = lrclib_id_from_url(url)?;
    let client = build_http_client()?;
    let api = format!("https://lrclib.net/api/get/{id}");
    let resp: serde_json::Value = client.get(&api).send().await.ok()?.json().await.ok()?;
    let text = resp["plainLyrics"]
        .as_str()
        .filter(|s| !s.trim().is_empty())?
        .to_string();
    let (sections, has_structure) = parse_sections(&text);
    if sections.is_empty() {
        return None;
    }
    Some(LyricsResult {
        artist: resp["artistName"].as_str().unwrap_or("").to_string(),
        artist_id: None,
        title: resp["trackName"].as_str().unwrap_or("").to_string(),
        album: resp["albumName"].as_str().map(String::from),
        release_date: None,
        thumbnail_url: None,
        sections,
        has_structure,
    })
}

/// Extract a numeric LRCLIB track id from a URL's path segments.
fn lrclib_id_from_url(url: &str) -> Option<u64> {
    url.trim_end_matches('/')
        .rsplit(['/', '?', '#'])
        .find_map(|seg| seg.parse::<u64>().ok())
}

/// Best-effort `(artist, title)` from a Genius page `<title>`, which looks like
/// `Artist – Song Lyrics | Genius`. Falls back to empty strings.
fn parse_genius_page_title(html: &str) -> (String, String) {
    let Some(start) = html.find("<title>") else {
        return (String::new(), String::new());
    };
    let rest = &html[start + "<title>".len()..];
    let Some(end) = rest.find("</title>") else {
        return (String::new(), String::new());
    };
    let raw = rest[..end].trim();
    // Strip the trailing " | Genius..." and the " Lyrics" marker.
    let core = raw.split('|').next().unwrap_or(raw).trim();
    let core = core.strip_suffix(" Lyrics").unwrap_or(core).trim();
    // Genius uses an en-dash between artist and title.
    if let Some((artist, title)) = core.split_once('–').or_else(|| core.split_once(" - ")) {
        (artist.trim().to_string(), title.trim().to_string())
    } else {
        (String::new(), core.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_provider_from_url() {
        assert_eq!(
            provider_for_url("https://genius.com/Dora-song-lyrics"),
            Some(LyricsProviderKind::Genius)
        );
        assert_eq!(
            provider_for_url("https://lrclib.net/api/get/12345"),
            Some(LyricsProviderKind::Lrclib)
        );
        assert_eq!(provider_for_url("https://example.com/x"), None);
    }

    #[test]
    fn extracts_lrclib_id() {
        assert_eq!(lrclib_id_from_url("https://lrclib.net/api/get/12345"), Some(12345));
        assert_eq!(lrclib_id_from_url("https://lrclib.net/api/get/12345/"), Some(12345));
        assert_eq!(lrclib_id_from_url("https://lrclib.net/no-id-here"), None);
    }

    #[test]
    fn parses_genius_title() {
        let html = "<html><head><title>Дора – Дорадура Lyrics | Genius Lyrics</title></head></html>";
        let (a, t) = parse_genius_page_title(html);
        assert_eq!(a, "Дора");
        assert_eq!(t, "Дорадура");
    }

    #[test]
    fn provider_kind_roundtrip() {
        for k in [LyricsProviderKind::Lrclib, LyricsProviderKind::Genius] {
            assert_eq!(LyricsProviderKind::from_id(k.id()), Some(k));
        }
    }
}
