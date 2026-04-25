//! Lyrics highlight extraction.
//!
//! Two strategies:
//! 1. **LLM** — asks Claude to pick line numbers of the most memorable lines.
//!    Uses line-number references to avoid content filter issues with explicit lyrics.
//! 2. **Structural fallback** — if LLM fails (no API key, content filter, timeout),
//!    extracts chorus/hook lines from already-parsed section structure.

use crate::core::llm;
use crate::lyrics::{LyricsSection, parse_sections};

/// Maximum lines to include in the highlight snippet.
const MAX_HIGHLIGHT_LINES: usize = 6;

/// Extract the most memorable lines from song lyrics.
///
/// Tries LLM first (Claude Haiku via line-number prompt), falls back to
/// structural extraction (chorus/hook sections) if LLM is unavailable.
///
/// Returns `None` if lyrics are too short or no meaningful highlights found.
pub async fn extract_highlights(artist: &str, title: &str, full_lyrics: &str) -> Option<String> {
    let non_empty_lines: Vec<&str> = full_lyrics.lines().filter(|l| !l.trim().is_empty()).collect();

    if non_empty_lines.len() <= 4 {
        return None;
    }

    // Try LLM first
    if let Some(result) = extract_via_llm(artist, title, &non_empty_lines).await {
        return Some(result);
    }

    // Fallback: structural extraction from chorus/hook
    extract_from_structure(full_lyrics)
}

/// LLM strategy: number each line, ask Claude to return line numbers only.
/// This avoids content filter issues since the model outputs numbers, not lyrics text.
async fn extract_via_llm(artist: &str, title: &str, lines: &[&str]) -> Option<String> {
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        return None;
    }

    // Number each line for reference
    let numbered: String = lines
        .iter()
        .enumerate()
        .take(200)
        .map(|(i, l)| format!("{}: {}", i + 1, l))
        .collect::<Vec<_>>()
        .join("\n");

    let prompt = format!(
        "Song: \"{title}\" by {artist}\n\n\
         Numbered lyrics:\n{numbered}\n\n\
         Which {MAX_HIGHLIGHT_LINES} lines are the most iconic, memorable, or emotionally striking? \
         These should be lines someone would quote — the hook, a chorus line, or a standout punchline.\n\n\
         Reply with ONLY the line numbers, comma-separated. Example: 3,7,12,15,22,28\n\
         Do NOT include any other text."
    );

    let response = llm::ask(llm::HAIKU, 60, &prompt).await?;

    // Parse comma-separated line numbers
    let selected: Vec<&str> = response
        .split([',', '\n'])
        .filter_map(|s| {
            let n: usize = s.trim().parse().ok()?;
            if n >= 1 && n <= lines.len() {
                Some(lines[n - 1].trim())
            } else {
                None
            }
        })
        .take(MAX_HIGHLIGHT_LINES)
        .collect();

    if selected.is_empty() {
        log::debug!("Lyrics highlights: LLM response unparseable: {:?}", response);
        return None;
    }

    Some(selected.join("\n"))
}

/// Structural fallback: extract chorus/hook lines without LLM.
///
/// Picks lines from the first Chorus or Hook section found in the lyrics.
/// If no structured sections exist, returns `None`.
fn extract_from_structure(full_lyrics: &str) -> Option<String> {
    let (sections, has_structure) = parse_sections(full_lyrics);

    if !has_structure || sections.is_empty() {
        return None;
    }

    // Priority: Chorus > Hook > Pre-Chorus > first section with content
    let priority_section = find_section_by_names(&sections, &["Chorus", "Hook"])
        .or_else(|| find_section_by_names(&sections, &["Pre-Chorus", "Refrain"]))
        .or_else(|| sections.first());

    let section = priority_section?;

    let lines: Vec<&str> = section
        .lines
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .take(MAX_HIGHLIGHT_LINES)
        .collect();

    if lines.is_empty() {
        return None;
    }

    Some(lines.join("\n"))
}

fn find_section_by_names<'a>(sections: &'a [LyricsSection], names: &[&str]) -> Option<&'a LyricsSection> {
    sections
        .iter()
        .find(|s| names.iter().any(|name| s.name.eq_ignore_ascii_case(name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_short_lyrics_returns_none() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(extract_highlights("Artist", "Song", "Line one\nLine two\nLine three"));
        assert!(result.is_none());
    }

    #[test]
    fn test_structural_fallback_chorus() {
        let lyrics = "\
[Verse 1]
Walking down the street
Thinking about life
Nothing really matters

[Chorus]
But I will always love you
No matter what they say
You are my everything

[Verse 2]
Another day goes by
The sun keeps on shining";

        let result = extract_from_structure(lyrics).unwrap();
        assert!(result.contains("But I will always love you"));
        assert!(result.contains("No matter what they say"));
        assert!(result.contains("You are my everything"));
    }

    #[test]
    fn test_structural_fallback_no_structure() {
        let lyrics = "Just some plain text\nwithout any sections\nnothing here";
        let result = extract_from_structure(lyrics);
        assert!(result.is_none());
    }

    #[test]
    fn test_structural_fallback_hook_priority() {
        let lyrics = "\
[Verse 1]
Some verse line

[Hook]
This is the hook baby
Remember this line

[Bridge]
A bridge line";

        let result = extract_from_structure(lyrics).unwrap();
        assert!(result.contains("This is the hook baby"));
    }
}
