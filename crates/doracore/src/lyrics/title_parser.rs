//! Heuristic parser to recover `(artist, track)` candidates from raw YouTube
//! video titles.
//!
//! Most YouTube uploads of commercial music follow predictable patterns:
//!
//! ```text
//! Eminem - Lose Yourself (Official Music Video)   → (Eminem, Lose Yourself)
//! LIL WAYNE - Mrs Officer ft. Bobby V             → (Lil Wayne, Mrs Officer)
//! Бузова — Мало половин (премьера 2024)          → (Бузова, Мало половин)
//! [Eminem] - [Lose Yourself] [HD]                 → (Eminem, Lose Yourself)
//! ```
//!
//! Re-upload channels invert the order: `Mrs Officer - Lil Wayne LYRICS`.
//! We can't disambiguate from the string alone, so the parser always returns
//! BOTH orderings as separate candidates and lets the lyrics-source layer
//! (Genius / LRCLIB) pick whichever resolves to a real track.
//!
//! This is **only** a heuristic. yt-dlp's `track` / `artist` JSON fields,
//! when present (YouTube Music auto-detection), are far more reliable —
//! prefer those when they're available.

use lazy_regex::{Lazy, Regex, lazy_regex};

/// Bracketed noise: `(Official Video)`, `[HD]`, `(Lyric Video)`, `(Audio)`,
/// `(Visualizer)`, `[Премьера N]`, `(2024 Remastered)`, etc.
///
/// Matches matched-pairs only — `(...)`, `[...]`, `{...}` — not bare
/// parentheses scattered through the title.
static BRACKETED_NOISE_RE: Lazy<Regex> = lazy_regex!(r"\([^)]*\)|\[[^\]]*\]|\{[^}]*\}");

/// Trailing `LYRICS` / `LYRIC VIDEO` / `OFFICIAL` markers after stripping
/// brackets (some uploaders write `Track Name LYRICS` without brackets).
static TRAILING_NOISE_RE: Lazy<Regex> =
    lazy_regex!(r"(?i)\s*\b(lyrics|lyric video|official(?:\s+\w+)?|премьера|preview|hd|4k|hq)\b\s*$");

/// `feat. X`, `ft. X`, `featuring X` — kept by default (some lyrics sites
/// match better with featured-artist info), but exposed as a separate
/// remover so callers can strip when needed.
static FEAT_RE: Lazy<Regex> = lazy_regex!(r"(?i)\s+(?:feat\.?|ft\.?|featuring)\s+[^\(\[\-–—]+");

/// Strip bracketed and trailing noise from a video title. Conservative —
/// keeps the core `Artist - Track` substring intact.
#[must_use]
pub fn clean_title(title: &str) -> String {
    let mut s = title.to_string();
    // Run the bracket regex multiple times to handle nested-ish noise like
    // `[Official] [HD] [4K]` where each stripped run can leave a trailing
    // chunk that the next pass can pick up.
    for _ in 0..3 {
        let next = BRACKETED_NOISE_RE.replace_all(&s, "").into_owned();
        if next == s {
            break;
        }
        s = next;
    }
    s = TRAILING_NOISE_RE.replace_all(&s, "").into_owned();
    s.trim().to_string()
}

/// Same as [`clean_title`] plus the `feat./ft.` chunk removed. Useful as a
/// secondary candidate for sources that index canonical track names without
/// featured-artist suffixes.
#[must_use]
pub fn clean_title_no_feat(title: &str) -> String {
    let cleaned = clean_title(title);
    FEAT_RE.replace_all(&cleaned, "").trim().to_string()
}

/// Split candidate on the first separator from this set, in priority order.
/// `–` (en-dash, U+2013) and `—` (em-dash, U+2014) are common in non-English
/// titles; ASCII `-` is the workhorse.
const SEPARATORS: &[&str] = &[" — ", " – ", " - "];

/// Extract ordered `(artist, track)` candidates from a YouTube title.
///
/// Returns up to 3 candidates ranked by likelihood:
///   1. Forward split: first chunk = artist, rest = track. Most common.
///   2. Reverse split: first chunk = track, rest = artist. Catches re-upload
///      channels (`Mrs Officer - Lil Wayne LYRICS`).
///   3. Cleaned-title-only fallback (artist empty, track = full cleaned title).
///
/// Returns an empty `Vec` only when `title` is whitespace-only after cleaning.
#[must_use]
pub fn extract_artist_track_candidates(title: &str) -> Vec<(String, String)> {
    let cleaned = clean_title(title);
    let cleaned_no_feat = clean_title_no_feat(title);
    if cleaned.trim().is_empty() {
        return Vec::new();
    }

    let mut out: Vec<(String, String)> = Vec::with_capacity(4);
    let mut push_unique = |a: &str, t: &str| {
        let pair = (a.trim().to_string(), t.trim().to_string());
        if pair.1.is_empty() {
            return;
        }
        if !out.contains(&pair) {
            out.push(pair);
        }
    };

    // Try splitter on the cleaned (with feat) variant.
    for sep in SEPARATORS {
        if let Some((left, right)) = cleaned.split_once(sep) {
            // 1. Forward split — assumed canonical.
            push_unique(left, right);
            // 2. Reverse split — for `Track - Artist` channels.
            push_unique(right, left);
            break; // pick the FIRST matching separator
        }
    }

    // Same against the no-feat variant — if feat info pulled, the canonical
    // track name often matches the lyrics index better.
    if cleaned_no_feat != cleaned {
        for sep in SEPARATORS {
            if let Some((left, right)) = cleaned_no_feat.split_once(sep) {
                push_unique(left, right);
                push_unique(right, left);
                break;
            }
        }
    }

    // Title-only fallback — no separator detected, or split-only candidates
    // exhausted. LRCLIB tolerates empty artist; Genius does not, but the
    // caller decides whether to use this last entry.
    push_unique("", &cleaned);

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_strips_official_brackets() {
        assert_eq!(
            clean_title("Eminem - Lose Yourself (Official Music Video)"),
            "Eminem - Lose Yourself"
        );
        assert_eq!(clean_title("Drake - One Dance (Official Audio)"), "Drake - One Dance");
        assert_eq!(clean_title("[Eminem] - [Lose Yourself]"), "-");
    }

    #[test]
    fn clean_strips_trailing_lyrics_marker() {
        assert_eq!(clean_title("MRS OFFICER - LIL WAYNE LYRICS"), "MRS OFFICER - LIL WAYNE");
        assert_eq!(clean_title("Some Track Lyric Video"), "Some Track");
    }

    #[test]
    fn clean_strips_quality_brackets() {
        assert_eq!(clean_title("Eminem - Lose Yourself [HD]"), "Eminem - Lose Yourself");
        assert_eq!(clean_title("Eminem - Lose Yourself [4K]"), "Eminem - Lose Yourself");
        assert_eq!(
            clean_title("Eminem - Lose Yourself [HD] [Official]"),
            "Eminem - Lose Yourself"
        );
    }

    #[test]
    fn clean_handles_cyrillic_premiere_marker() {
        assert_eq!(
            clean_title("Бузова — Мало половин (премьера 2024)"),
            "Бузова — Мало половин"
        );
    }

    #[test]
    fn clean_keeps_inner_punctuation() {
        // Don't accidentally eat hyphens that ARE part of the artist or track.
        assert_eq!(
            clean_title("twenty one pilots - Stressed Out (Official Video)"),
            "twenty one pilots - Stressed Out"
        );
    }

    #[test]
    fn clean_no_feat_strips_featuring() {
        assert_eq!(
            clean_title_no_feat("Lil Wayne - Mrs Officer ft. Bobby V (Official Video)"),
            "Lil Wayne - Mrs Officer"
        );
        assert_eq!(
            clean_title_no_feat("Drake - Toosie Slide feat. Future"),
            "Drake - Toosie Slide"
        );
    }

    #[test]
    fn extract_canonical_artist_track_split() {
        let cands = extract_artist_track_candidates("Eminem - Lose Yourself (Official Music Video)");
        // Forward split first.
        assert_eq!(cands[0], ("Eminem".to_string(), "Lose Yourself".to_string()));
        // Reverse split second (for re-upload-channel disambiguation).
        assert_eq!(cands[1], ("Lose Yourself".to_string(), "Eminem".to_string()));
        // Title-only fallback.
        assert!(cands.iter().any(|(a, t)| a.is_empty() && t == "Eminem - Lose Yourself"));
    }

    #[test]
    fn extract_handles_em_dash() {
        let cands = extract_artist_track_candidates("Бузова — Мало половин");
        assert_eq!(cands[0], ("Бузова".to_string(), "Мало половин".to_string()));
        assert_eq!(cands[1], ("Мало половин".to_string(), "Бузова".to_string()));
    }

    #[test]
    fn extract_inverted_reupload_channel_yields_both_orders() {
        // Channel uploads as `Track - Artist LYRICS` instead of canonical
        // `Artist - Track`. Reverse-split candidate catches this.
        let cands = extract_artist_track_candidates("MRS OFFICER - LIL WAYNE LYRICS");
        // Forward split: ("MRS OFFICER", "LIL WAYNE")
        assert!(cands.contains(&("MRS OFFICER".to_string(), "LIL WAYNE".to_string())));
        // Reverse split: ("LIL WAYNE", "MRS OFFICER")
        assert!(cands.contains(&("LIL WAYNE".to_string(), "MRS OFFICER".to_string())));
    }

    #[test]
    fn extract_yields_no_feat_variant_separately() {
        // `feat. ...` variant should produce additional candidates with
        // featured-artist info pulled, since Genius/LRCLIB sometimes index
        // canonical track names without it.
        let cands = extract_artist_track_candidates("Lil Wayne - Mrs Officer ft. Bobby V");
        // With feat (forward split).
        assert!(cands.contains(&("Lil Wayne".to_string(), "Mrs Officer ft. Bobby V".to_string())));
        // Without feat — preferred match for lyrics indexes.
        assert!(cands.contains(&("Lil Wayne".to_string(), "Mrs Officer".to_string())));
    }

    #[test]
    fn extract_falls_back_to_title_only() {
        let cands = extract_artist_track_candidates("ProGorlovkaTV ASMR slime ");
        // No separator → only title-only candidate.
        assert!(
            cands
                .iter()
                .any(|(a, t)| a.is_empty() && t == "ProGorlovkaTV ASMR slime")
        );
    }

    #[test]
    fn extract_empty_input_yields_empty_vec() {
        assert!(extract_artist_track_candidates("").is_empty());
        assert!(extract_artist_track_candidates("   ").is_empty());
        assert!(extract_artist_track_candidates("(Official) [HD]").is_empty());
    }

    #[test]
    fn extract_picks_first_separator_only() {
        // Don't double-split on multiple separators in the title.
        let cands = extract_artist_track_candidates("A - B - C");
        // First " - " split: artist="A", track="B - C"
        assert!(cands.contains(&("A".to_string(), "B - C".to_string())));
    }
}
