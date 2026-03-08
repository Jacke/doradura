//! LLM-powered lyrics highlight extraction.
//!
//! Given full song lyrics, uses Claude to pick the most iconic/memorable lines
//! (hook, chorus, punchline). Returns a short snippet suitable for sending
//! alongside the audio file in Telegram.

use crate::core::llm;

/// Maximum lines to include in the highlight snippet.
const MAX_HIGHLIGHT_LINES: usize = 6;

/// Extract the most memorable lines from song lyrics using an LLM.
///
/// Returns `None` if:
/// - `ANTHROPIC_API_KEY` is not set
/// - The LLM call fails
/// - Lyrics are too short to highlight (≤4 lines)
///
/// The returned string is plain text, ready to embed in a Telegram message.
pub async fn extract_highlights(artist: &str, title: &str, full_lyrics: &str) -> Option<String> {
    let line_count = full_lyrics.lines().filter(|l| !l.trim().is_empty()).count();
    if line_count <= 4 {
        return None;
    }

    // Truncate very long lyrics to avoid wasting tokens
    let lyrics_for_prompt: String = full_lyrics.lines().take(200).collect::<Vec<_>>().join("\n");

    let prompt = format!(
        "Song: \"{title}\" by {artist}\n\n\
         Lyrics:\n{lyrics_for_prompt}\n\n\
         Pick the {MAX_HIGHLIGHT_LINES} most iconic, memorable, or emotionally striking lines from this song. \
         These should be the lines someone would quote or remember — the hook, a powerful chorus line, or a standout punchline.\n\n\
         Rules:\n\
         - Return ONLY the selected lines, one per line, exactly as they appear in the lyrics (no modifications)\n\
         - Do NOT add numbering, labels, explanations, or quotes\n\
         - If the song has fewer than {MAX_HIGHLIGHT_LINES} memorable lines, return fewer\n\
         - Prefer lines from the chorus/hook if they exist"
    );

    let response = llm::ask(llm::HAIKU, 300, &prompt).await?;

    // Validate: each returned line should appear in the original lyrics
    let validated: Vec<&str> = response
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .filter(|line| {
            // Fuzzy match: line should appear in lyrics (case-insensitive)
            let lower = line.to_lowercase();
            full_lyrics.to_lowercase().contains(&lower)
        })
        .take(MAX_HIGHLIGHT_LINES)
        .collect();

    if validated.is_empty() {
        // LLM hallucinated lines not in lyrics — return the raw response
        // but cap it to MAX_HIGHLIGHT_LINES lines
        let fallback: Vec<&str> = response
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .take(MAX_HIGHLIGHT_LINES)
            .collect();

        if fallback.is_empty() {
            return None;
        }
        return Some(fallback.join("\n"));
    }

    Some(validated.join("\n"))
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
}
