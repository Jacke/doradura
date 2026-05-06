//! Lyrics fetching and parsing module.
//!
//! Fetches lyrics from LRCLIB (free, no auth) with optional Genius fallback
//! for structured lyrics with verse/chorus sections.
//!
//! If `GENIUS_CLIENT_TOKEN` env var is set, Genius is tried first for better
//! structure (especially for rap/hip-hop). Otherwise falls back to LRCLIB.

pub mod highlights;
pub mod title_parser;

use lazy_regex::{Lazy, Regex, lazy_regex};
use serde::{Deserialize, Serialize};

/// Matches common song section headers like [Verse 1], [Chorus], [Bridge], etc.
/// (?m) enables multiline mode so ^ and $ match per line (needed for is_match on whole text).
static SECTION_RE: Lazy<Regex> = lazy_regex!(
    r"(?im)^\[((?:verse|chorus|bridge|pre[\-\s]?chorus|post[\-\s]?chorus|hook|intro|outro|interlude|refrain|instrumental|breakdown|coda|skit|drop|spoken|transition|banger|trap|rap)\s*\d*)\]$"
);

static HTML_TAG_RE: Lazy<Regex> = lazy_regex!(r"<[^>]+>");

/// A single song entry in the artist's list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtistSong {
    pub id: u64,
    pub title: String,
    pub artist: String,
    pub thumbnail_url: Option<String>,
}

/// A single labeled section of a song (e.g. Verse 1, Chorus, Bridge).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LyricsSection {
    pub name: String,
    pub lines: Vec<String>,
}

impl LyricsSection {
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }
}

/// Parsed lyrics result, with or without detected section structure.
#[derive(Debug, Clone)]
pub struct LyricsResult {
    pub artist: String,
    pub artist_id: Option<u64>,
    pub title: String,
    pub album: Option<String>,
    pub release_date: Option<String>,
    pub thumbnail_url: Option<String>,
    pub sections: Vec<LyricsSection>,
    /// True when [Verse]/[Chorus] markers were found and parsed.
    pub has_structure: bool,
}

impl LyricsResult {
    pub fn get_section(&self, idx: usize) -> Option<&LyricsSection> {
        self.sections.get(idx)
    }

    /// Full text of all sections joined together.
    pub fn all_text(&self) -> String {
        self.sections
            .iter()
            .map(|s| {
                if self.has_structure {
                    format!("[{}]\n{}", s.name, s.text())
                } else {
                    s.text()
                }
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// Parse artist and title from a display_title ("Artist - Title" or just "Title").
/// Uses rfind to split on the LAST " - " separator — avoids misparse when title
/// has prefixes like "Music Audio - Nirvana - Rape Me".
/// Returns ("", display_title) when no separator is found.
pub fn parse_artist_title(display_title: &str) -> (&str, &str) {
    if let Some(pos) = display_title.rfind(" - ") {
        (&display_title[..pos], &display_title[pos + 3..])
    } else {
        ("", display_title)
    }
}

/// Parse plain lyrics text into sections.
/// Returns (sections, has_structure) — has_structure is true if section markers were found.
pub fn parse_sections(text: &str) -> (Vec<LyricsSection>, bool) {
    if !SECTION_RE.is_match(text) {
        let lines: Vec<String> = text
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        if lines.is_empty() {
            return (vec![], false);
        }
        return (
            vec![LyricsSection {
                name: "Lyrics".to_string(),
                lines,
            }],
            false,
        );
    }

    let mut sections: Vec<LyricsSection> = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_lines: Vec<String> = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(caps) = SECTION_RE.captures(trimmed) {
            // Flush previous section
            if let Some(name) = current_name.take()
                && !current_lines.is_empty()
            {
                sections.push(LyricsSection {
                    name,
                    lines: current_lines.clone(),
                });
                current_lines.clear();
            }
            let raw = caps.get(1).map(|m| m.as_str()).unwrap_or(trimmed);
            // Normalize to title case regardless of input case (CHORUS → Chorus)
            current_name = Some(title_case(&raw.trim().to_lowercase()));
        } else if !trimmed.is_empty() {
            // Only collect lines that are inside a section
            if current_name.is_some() {
                current_lines.push(trimmed.to_string());
            }
        }
    }
    // Flush last section
    if let Some(name) = current_name
        && !current_lines.is_empty()
    {
        sections.push(LyricsSection {
            name,
            lines: current_lines,
        });
    }

    if sections.is_empty() {
        // Regex matched headers but all sections were empty — fall back to plain
        let lines: Vec<String> = text
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        return (
            vec![LyricsSection {
                name: "Lyrics".to_string(),
                lines,
            }],
            false,
        );
    }

    (sections, true)
}

/// Split unstructured lyrics into pseudo-sections of ~8 lines each so the
/// picker can offer multiple chunks. Source-of-truth lyrics from LRCLIB
/// (and some Genius pages) lack `[Verse]/[Chorus]` markers, leaving us with
/// a single 100-line "Lyrics" section — useless for picking. We chunk by
/// fixed line count rather than blank-line stanzas because `parse_sections`
/// already strips blanks before we get here.
pub fn auto_segment_unstructured(lines: &[String]) -> Vec<LyricsSection> {
    const CHUNK: usize = 8;
    if lines.len() <= CHUNK {
        return vec![LyricsSection {
            name: "Lyrics".to_string(),
            lines: lines.to_vec(),
        }];
    }
    lines
        .chunks(CHUNK)
        .enumerate()
        .map(|(i, chunk)| LyricsSection {
            name: format!("Part {}", i + 1),
            lines: chunk.to_vec(),
        })
        .collect()
}

/// Build a compact button label for a section: `"{name} · {first-line preview}"`,
/// capped at `max_chars`. Falls back to `name` alone when the section has no
/// lines or the preview adds no information.
pub fn section_button_label(s: &LyricsSection, max_chars: usize) -> String {
    let preview = s
        .lines
        .iter()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim())
        .unwrap_or("");
    if preview.is_empty() {
        return s.name.clone();
    }
    let combined = format!("{} · {}", s.name, preview);
    let chars: Vec<char> = combined.chars().collect();
    if chars.len() <= max_chars {
        return combined;
    }
    let truncated: String = chars.iter().take(max_chars.saturating_sub(1)).collect();
    format!("{}…", truncated)
}

fn title_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut cap_next = true;
    for c in s.chars() {
        if c.is_whitespace() || c == '-' {
            cap_next = true;
            result.push(c);
        } else if cap_next {
            result.extend(c.to_uppercase());
            cap_next = false;
        } else {
            result.push(c);
        }
    }
    result
}

fn build_http_client() -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .user_agent("doradura-bot/1.0")
        .build()
        .ok()
}

type FullMetadata = (
    String,
    Option<u64>,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
);

/// Fetch plain lyrics from LRCLIB (primary free source, no auth required).
async fn fetch_from_lrclib(artist: &str, title: &str) -> Option<FullMetadata> {
    let client = build_http_client()?;
    let url = if artist.is_empty() {
        format!("https://lrclib.net/api/search?q={}", urlencoding::encode(title))
    } else {
        format!(
            "https://lrclib.net/api/search?artist_name={}&track_name={}",
            urlencoding::encode(artist),
            urlencoding::encode(title),
        )
    };

    let resp: serde_json::Value = client.get(&url).send().await.ok()?.json().await.ok()?;
    let arr = resp.as_array()?;
    let first = arr.first()?;

    let artist = first["artistName"].as_str().unwrap_or("Unknown Artist").to_string();
    let title = first["trackName"].as_str().unwrap_or("Unknown Title").to_string();
    let album = first["albumName"].as_str().map(String::from);
    let lyrics = first["plainLyrics"]
        .as_str()
        .filter(|s| !s.trim().is_empty())?
        .to_string();

    Some((artist, None, title, album, None, None, lyrics))
}

/// Fetch structured lyrics from Genius (requires Genius access token).
/// Returns (artist, artist_id, title, album, release_date, thumb, raw_text)
async fn fetch_from_genius(artist: &str, title: &str, token: &str) -> Option<FullMetadata> {
    let client = build_http_client()?;

    let query = if artist.is_empty() {
        title.to_string()
    } else {
        format!("{} {}", artist, title)
    };

    let search_url = format!("https://api.genius.com/search?q={}", urlencoding::encode(&query));

    let resp: serde_json::Value = client
        .get(&search_url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    let first_hit = resp["response"]["hits"].as_array()?.first()?;
    let result = &first_hit["result"];

    let song_url = result["url"].as_str()?.to_string();
    let found_artist = result["primary_artist"]["name"]
        .as_str()
        .unwrap_or("Unknown Artist")
        .to_string();
    let found_artist_id = result["primary_artist"]["id"].as_u64();
    let found_title = result["title"].as_str().unwrap_or("Unknown Title").to_string();
    let thumbnail_url = result["header_image_thumbnail_url"].as_str().map(String::from);
    let release_date = result["release_date_for_display"].as_str().map(String::from);

    log::info!("Lyrics: Genius scraping {}", song_url);

    // LOW-09: cap the response body to 5 MiB to prevent memory exhaustion if
    // Genius returns an unexpectedly large or malicious response.
    const MAX_GENIUS_HTML_BYTES: usize = 5 * 1024 * 1024; // 5 MiB

    let resp = client
        .get(&song_url)
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await
        .ok()?;

    // Reject if Content-Length already signals an oversized body.
    if let Some(content_length) = resp.content_length()
        && content_length as usize > MAX_GENIUS_HTML_BYTES
    {
        log::warn!(
            "Lyrics: Genius page too large ({} bytes), skipping {}",
            content_length,
            song_url
        );
        return None;
    }

    let bytes = resp.bytes().await.ok()?;
    if bytes.len() > MAX_GENIUS_HTML_BYTES {
        log::warn!(
            "Lyrics: Genius page body too large ({} bytes), skipping {}",
            bytes.len(),
            song_url
        );
        return None;
    }
    let html = String::from_utf8_lossy(&bytes).into_owned();

    let lyrics = parse_genius_html(&html)?;
    Some((
        found_artist,
        found_artist_id,
        found_title,
        None,
        release_date,
        thumbnail_url,
        lyrics,
    ))
}

/// Fetch list of songs by artist ID from Genius.
pub async fn fetch_artist_songs(artist_id: u64, token: &str, page: u32) -> Option<Vec<ArtistSong>> {
    let client = build_http_client()?;

    let url = format!(
        "https://api.genius.com/artists/{}/songs?sort=popularity&per_page=20&page={}",
        artist_id, page
    );

    let resp: serde_json::Value = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    let songs = resp["response"]["songs"].as_array()?;
    let mut results = Vec::new();

    for s in songs {
        results.push(ArtistSong {
            id: s["id"].as_u64()?,
            title: s["title"].as_str()?.to_string(),
            artist: s["primary_artist"]["name"].as_str()?.to_string(),
            thumbnail_url: s["header_image_thumbnail_url"].as_str().map(String::from),
        });
    }

    Some(results)
}

/// Search for songs/artists and return a list of matches.
pub async fn fetch_search_results(query: &str, token: &str, page: u32) -> Option<Vec<ArtistSong>> {
    let client = build_http_client()?;
    let url = format!(
        "https://api.genius.com/search?q={}&per_page=20&page={}",
        urlencoding::encode(query),
        page
    );

    let resp: serde_json::Value = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    let hits = resp["response"]["hits"].as_array()?;
    let mut results = Vec::new();

    for h in hits {
        let result = &h["result"];
        results.push(ArtistSong {
            id: result["id"].as_u64()?,
            title: result["title"].as_str()?.to_string(),
            artist: result["primary_artist"]["name"].as_str()?.to_string(),
            thumbnail_url: result["header_image_thumbnail_url"].as_str().map(String::from),
        });
    }

    Some(results)
}

/// Search for an artist ID by name using Genius search.
pub async fn fetch_artist_id(artist_name: &str, token: &str) -> Option<u64> {
    let client = build_http_client()?;

    // Try a direct search first
    let url = format!("https://api.genius.com/search?q={}", urlencoding::encode(artist_name));
    let resp: serde_json::Value = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    let hits = resp["response"]["hits"].as_array()?;

    // 1. Look for a strong match in hits
    for hit in hits {
        let result = &hit["result"];
        let found_name = result["primary_artist"]["name"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase();
        let target = artist_name.to_lowercase();
        if found_name == target || found_name.contains(&target) || target.contains(&found_name) {
            return result["primary_artist"]["id"].as_u64();
        }
    }

    // 2. Fallback to the first hit if any
    if let Some(first) = hits.first() {
        return first["result"]["primary_artist"]["id"].as_u64();
    }

    None
}

fn parse_genius_html(html: &str) -> Option<String> {
    use select::document::Document;
    use select::predicate::Attr;

    let doc = Document::from(html);
    let mut full_text = String::new();

    for node in doc.find(Attr("data-lyrics-container", "true")) {
        let inner = node.inner_html();
        // Preserve line breaks before stripping tags
        let with_newlines = inner
            .replace("<br/>", "\n")
            .replace("<br>", "\n")
            .replace("<br />", "\n");
        // Strip remaining HTML tags
        let stripped = HTML_TAG_RE.replace_all(&with_newlines, "");
        // Decode common HTML entities
        let decoded = decode_html_entities(&stripped);

        // Skip blocks that are clearly metadata (contributors list, translations header)
        let lower = decoded.trim().to_lowercase();
        if lower.contains("contributors") || lower.len() < 5 {
            continue;
        }

        full_text.push_str(&decoded);
        full_text.push('\n');
    }

    let trimmed = full_text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn decode_html_entities(s: &str) -> String {
    s.replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
}

/// Fetch lyrics for artist + title.
///
/// Strategy:
/// 1. Genius (if token provided or `GENIUS_CLIENT_TOKEN` is set) — best structure for rap/pop
/// 2. LRCLIB — free, no auth, good coverage
///
/// Returns `None` if no source has lyrics for this track.
pub async fn fetch_lyrics(artist: &str, title: &str, token: Option<&str>) -> Option<LyricsResult> {
    let genius_token = token
        .map(|s| s.to_string())
        .or_else(|| crate::core::config::GENIUS_CLIENT_TOKEN.as_ref().cloned());

    // Try Genius first when token is configured
    if let Some(t) = genius_token {
        match fetch_from_genius(artist, title, &t).await {
            Some((found_artist, found_artist_id, found_title, found_album, release_date, thumbnail_url, text)) => {
                let (sections, has_structure) = parse_sections(&text);
                if !sections.is_empty() {
                    log::info!("Lyrics: Genius → '{} - {}'", found_artist, found_title);
                    return Some(LyricsResult {
                        artist: found_artist,
                        artist_id: found_artist_id,
                        title: found_title,
                        album: found_album,
                        release_date,
                        thumbnail_url,
                        sections,
                        has_structure,
                    });
                }
            }
            None => {
                log::warn!(
                    "Lyrics: Genius failed for '{} - {}', falling back to LRCLIB",
                    artist,
                    title
                );
            }
        }
    }

    // LRCLIB fallback
    match fetch_from_lrclib(artist, title).await {
        Some((found_artist, found_artist_id, found_title, found_album, release_date, thumbnail_url, text)) => {
            let (sections, has_structure) = parse_sections(&text);
            if !sections.is_empty() {
                log::info!("Lyrics: LRCLIB → '{} - {}'", found_artist, found_title);
                return Some(LyricsResult {
                    artist: found_artist,
                    artist_id: found_artist_id,
                    title: found_title,
                    album: found_album,
                    release_date,
                    thumbnail_url,
                    sections,
                    has_structure,
                });
            }
            log::warn!(
                "Lyrics: LRCLIB returned empty text for '{} - {}'",
                found_artist,
                found_title
            );
        }
        None => {
            log::warn!("Lyrics: LRCLIB no results for '{} - {}'", artist, title);
        }
    }

    None
}

/// Smart wrapper around [`fetch_lyrics`] that walks several candidate
/// `(artist, track)` pairs derived from the raw video title before giving up.
///
/// Recovery cascade — first hit wins, abort the rest:
///
/// 1. **Channel + cleaned title** — the original `(artist, title)` pair, but
///    with bracket noise (`(Official Video)`, `[HD]`, …) and trailing
///    `LYRICS` markers stripped. Catches the common case where the channel
///    actually IS the artist (e.g. official VEVO uploads).
/// 2. **Title-parsed candidates** — see
///    [`title_parser::extract_artist_track_candidates`]. Both forward
///    (`Artist - Track`) and reverse (`Track - Artist`) splits, plus the
///    `feat./ft.`-stripped variant. This is the layer that fixes re-upload
///    channels like `musiko lyriko - MRS OFFICER LIL WAYNE LYRICS`.
/// 3. **Title-only** — last resort. LRCLIB tolerates empty `artist`; Genius
///    skips this candidate internally.
///
/// Returns `None` only if every candidate fails. Logs each attempt at INFO
/// so production can see the cascade in action.
pub async fn fetch_lyrics_smart(channel: &str, title: &str, token: Option<&str>) -> Option<LyricsResult> {
    let cleaned_title = title_parser::clean_title(title);

    // Pass 1: channel + cleaned title (original behaviour, just denoised).
    if !channel.trim().is_empty() && !cleaned_title.is_empty() {
        log::info!("Lyrics: pass 1 — channel='{}', title='{}'", channel, cleaned_title);
        if let Some(lyr) = fetch_lyrics(channel, &cleaned_title, token).await {
            return Some(lyr);
        }
    }

    // Pass 2: title-parsed candidates. Skip pairs whose artist matches the
    // channel we already tried — no point re-running the same query.
    for (cand_artist, cand_track) in title_parser::extract_artist_track_candidates(title) {
        if cand_artist.trim().eq_ignore_ascii_case(channel.trim()) && cand_track.eq(&cleaned_title) {
            continue;
        }
        // Title-only candidate handled separately as the explicit final pass.
        if cand_artist.is_empty() {
            continue;
        }
        log::info!(
            "Lyrics: pass 2 — candidate artist='{}', track='{}'",
            cand_artist,
            cand_track
        );
        if let Some(lyr) = fetch_lyrics(&cand_artist, &cand_track, token).await {
            return Some(lyr);
        }
    }

    // Pass 3: title-only (LRCLIB-only effectively — Genius internally skips
    // empty artist).
    if !cleaned_title.is_empty() {
        log::info!("Lyrics: pass 3 — title-only='{}'", cleaned_title);
        if let Some(lyr) = fetch_lyrics("", &cleaned_title, token).await {
            return Some(lyr);
        }
    }

    log::info!(
        "Lyrics: smart fetch exhausted all passes for channel='{}', title='{}'",
        channel,
        title
    );
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_artist_title_with_separator() {
        let (artist, title) = parse_artist_title("Eminem - Lose Yourself");
        assert_eq!(artist, "Eminem");
        assert_eq!(title, "Lose Yourself");
    }

    #[test]
    fn test_parse_artist_title_no_separator() {
        let (artist, title) = parse_artist_title("Yesterday");
        assert_eq!(artist, "");
        assert_eq!(title, "Yesterday");
    }

    #[test]
    fn test_parse_artist_title_multiple_dashes() {
        let (artist, title) = parse_artist_title("Jay-Z - 99 Problems");
        assert_eq!(artist, "Jay-Z");
        assert_eq!(title, "99 Problems");
    }

    #[test]
    fn test_parse_sections_structured() {
        let text = "[Verse 1]\nLine one\nLine two\n\n[Chorus]\nRefrain here\nRefrain more";
        let (sections, has_structure) = parse_sections(text);
        assert!(has_structure);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].name, "Verse 1");
        assert_eq!(sections[0].lines, vec!["Line one", "Line two"]);
        assert_eq!(sections[1].name, "Chorus");
    }

    #[test]
    fn test_parse_sections_no_markers() {
        let text = "Line one\nLine two\nLine three";
        let (sections, has_structure) = parse_sections(text);
        assert!(!has_structure);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].name, "Lyrics");
        assert_eq!(sections[0].lines.len(), 3);
    }

    #[test]
    fn test_parse_sections_case_insensitive() {
        let text = "[CHORUS]\nLine\n[verse 2]\nLine2";
        let (sections, has_structure) = parse_sections(text);
        assert!(has_structure);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].name, "Chorus");
        assert_eq!(sections[1].name, "Verse 2");
    }

    #[test]
    fn test_parse_sections_pre_chorus() {
        let text = "[Verse 1]\nA\n[Pre-Chorus]\nB\n[Chorus]\nC";
        let (sections, has_structure) = parse_sections(text);
        assert!(has_structure);
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[1].name, "Pre-Chorus");
    }

    #[test]
    fn test_all_text_with_structure() {
        let result = LyricsResult {
            artist: "Artist".to_string(),
            artist_id: None,
            title: "Title".to_string(),
            album: None,
            release_date: None,
            thumbnail_url: None,
            sections: vec![
                LyricsSection {
                    name: "Verse 1".to_string(),
                    lines: vec!["Hello".to_string()],
                },
                LyricsSection {
                    name: "Chorus".to_string(),
                    lines: vec!["World".to_string()],
                },
            ],
            has_structure: true,
        };
        let text = result.all_text();
        assert!(text.contains("[Verse 1]"));
        assert!(text.contains("[Chorus]"));
    }

    #[test]
    fn test_decode_html_entities() {
        assert_eq!(decode_html_entities("it&#x27;s"), "it's");
        assert_eq!(decode_html_entities("&amp;"), "&");
    }

    #[test]
    fn test_auto_segment_unstructured_short() {
        let lines: Vec<String> = (0..5).map(|i| format!("line {}", i)).collect();
        let segs = auto_segment_unstructured(&lines);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].name, "Lyrics");
        assert_eq!(segs[0].lines.len(), 5);
    }

    #[test]
    fn test_auto_segment_unstructured_chunks() {
        let lines: Vec<String> = (0..20).map(|i| format!("line {}", i)).collect();
        let segs = auto_segment_unstructured(&lines);
        // 20 / 8 = 3 chunks (8 + 8 + 4)
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].name, "Part 1");
        assert_eq!(segs[0].lines.len(), 8);
        assert_eq!(segs[1].name, "Part 2");
        assert_eq!(segs[1].lines.len(), 8);
        assert_eq!(segs[2].name, "Part 3");
        assert_eq!(segs[2].lines.len(), 4);
    }

    #[test]
    fn test_section_button_label_short() {
        let s = LyricsSection {
            name: "Verse 1".to_string(),
            lines: vec!["Yo listen up".to_string()],
        };
        assert_eq!(section_button_label(&s, 32), "Verse 1 · Yo listen up");
    }

    #[test]
    fn test_section_button_label_truncates() {
        let s = LyricsSection {
            name: "Verse 1".to_string(),
            lines: vec!["The quick brown fox jumps over the lazy dog".to_string()],
        };
        let label = section_button_label(&s, 24);
        assert!(label.chars().count() <= 24);
        assert!(label.ends_with('…'));
        assert!(label.starts_with("Verse 1 · "));
    }

    #[test]
    fn test_section_button_label_no_preview() {
        let s = LyricsSection {
            name: "Outro".to_string(),
            lines: vec![],
        };
        assert_eq!(section_button_label(&s, 32), "Outro");
    }
}
