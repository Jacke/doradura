//! ISO-3166-1 alpha-2 country code → flag emoji helper.
//!
//! Used by the Info feature (geo-availability card) to render
//! `["RU", "BY", "KZ"]` as `🇷🇺 RU, 🇧🇾 BY, 🇰🇿 KZ`.
//!
//! Generated programmatically from the regional indicator base (`U+1F1E6`):
//! each ASCII letter `A-Z` maps to `U+1F1E6 + (letter - 'A')`. Joining
//! two regional indicators produces the flag glyph for that country.
//!
//! Returns `🏴` (waving black flag) for invalid / unknown codes — matches
//! the user-facing "unknown region" affordance without resorting to text.
//!
//! Reference: <https://en.wikipedia.org/wiki/Regional_indicator_symbol>.

/// Convert an ISO-3166-1 alpha-2 country code to its flag emoji.
///
/// Accepts upper- or lower-case (`"ru"`, `"RU"`, `"Ru"`); returns the
/// regional-indicator pair. Falls back to `🏴` for malformed input
/// (length ≠ 2, non-ASCII, non-alpha).
///
/// # Examples
///
/// ```
/// use doracore::core::country::country_flag;
///
/// assert_eq!(country_flag("RU"), "🇷🇺");
/// assert_eq!(country_flag("us"), "🇺🇸");
/// assert_eq!(country_flag("XX"), "🇽🇽"); // mapping is purely mechanical — invalid codes still render
/// assert_eq!(country_flag("X"),  "🏴");  // length mismatch → fallback
/// assert_eq!(country_flag("RU1"), "🏴"); // length mismatch → fallback
/// assert_eq!(country_flag(""),   "🏴");
/// ```
#[must_use]
pub fn country_flag(code: &str) -> String {
    let bytes = code.as_bytes();
    if bytes.len() != 2 {
        return "🏴".to_string();
    }
    let a = bytes[0].to_ascii_uppercase();
    let b = bytes[1].to_ascii_uppercase();
    if !(a.is_ascii_uppercase() && b.is_ascii_uppercase()) {
        return "🏴".to_string();
    }
    let base: u32 = 0x1F1E6;
    let a_off = (a - b'A') as u32;
    let b_off = (b - b'A') as u32;
    // Safe: A-Z maps to 0..26, base + 25 = 0x1F1FF, both within Plane 1 PUA.
    let ch_a = char::from_u32(base + a_off).unwrap_or('🏴');
    let ch_b = char::from_u32(base + b_off).unwrap_or('🏴');
    let mut s = String::with_capacity(8);
    s.push(ch_a);
    s.push(ch_b);
    s
}

/// Format a list of country codes as `🇷🇺 RU, 🇧🇾 BY, 🇰🇿 KZ`.
///
/// Empty input returns an empty string — caller decides what to render
/// for "no countries" (e.g. "Доступно везде").
#[must_use]
pub fn format_country_list(codes: &[String]) -> String {
    codes
        .iter()
        .map(|c| format!("{} {}", country_flag(c), c.to_uppercase()))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_for_known_codes() {
        assert_eq!(country_flag("RU"), "🇷🇺");
        assert_eq!(country_flag("US"), "🇺🇸");
        assert_eq!(country_flag("BY"), "🇧🇾");
        assert_eq!(country_flag("KZ"), "🇰🇿");
        assert_eq!(country_flag("DE"), "🇩🇪");
        assert_eq!(country_flag("FR"), "🇫🇷");
    }

    #[test]
    fn flag_is_case_insensitive() {
        assert_eq!(country_flag("ru"), "🇷🇺");
        assert_eq!(country_flag("Ru"), "🇷🇺");
        assert_eq!(country_flag("rU"), "🇷🇺");
    }

    #[test]
    fn flag_falls_back_for_malformed_input() {
        assert_eq!(country_flag(""), "🏴");
        assert_eq!(country_flag("R"), "🏴");
        assert_eq!(country_flag("RUS"), "🏴");
        assert_eq!(country_flag("R1"), "🏴");
        assert_eq!(country_flag("12"), "🏴");
    }

    #[test]
    fn format_list_empty_is_empty() {
        let empty: &[String] = &[];
        assert_eq!(format_country_list(empty), "");
    }

    #[test]
    fn format_list_joins_with_comma() {
        let codes = vec!["RU".to_string(), "BY".to_string(), "KZ".to_string()];
        assert_eq!(format_country_list(&codes), "🇷🇺 RU, 🇧🇾 BY, 🇰🇿 KZ");
    }

    #[test]
    fn format_list_uppercases_codes() {
        let codes = vec!["ru".to_string(), "by".to_string()];
        assert_eq!(format_country_list(&codes), "🇷🇺 RU, 🇧🇾 BY");
    }
}
