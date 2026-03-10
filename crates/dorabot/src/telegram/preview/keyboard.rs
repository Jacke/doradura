use crate::telegram::types::VideoFormatInfo;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub fn keyboard_stats(keyboard: &InlineKeyboardMarkup) -> (usize, usize) {
    let rows = keyboard.inline_keyboard.len();
    let buttons = keyboard.inline_keyboard.iter().map(|row| row.len()).sum();
    (rows, buttons)
}

/// Creates a standard keyboard with a download button
///
/// Used as fallback when the format list is unavailable
///
/// # Parameters
/// - `default_format` - file format (mp3, mp4, srt, txt)
/// - `default_quality` - video quality (mp4 only: "1080p", "720p", "480p", "360p", "best")
/// - `url_id` - URL ID in cache
pub fn create_fallback_keyboard(
    default_format: &str,
    default_quality: Option<&str>,
    url_id: &str,
    audio_bitrate: Option<&str>,
    is_youtube: bool,
    burn_sub_lang: Option<&str>,
) -> InlineKeyboardMarkup {
    log::debug!(
        "Creating fallback preview keyboard (format={}, quality={:?}, url_id={})",
        default_format,
        default_quality,
        url_id
    );
    let mp3_label = audio_bitrate
        .map(|bitrate| format!("MP3 {}", bitrate))
        .unwrap_or_else(|| "MP3".to_string());

    // Build button text based on format and quality
    let (button_text, callback_data) = match default_format {
        "mp4" => {
            // For video show quality
            let (quality_display, quality_for_callback) = match default_quality {
                Some("1080p") => ("1080p", "1080p"),
                Some("720p") => ("720p", "720p"),
                Some("480p") => ("480p", "480p"),
                Some("360p") => ("360p", "360p"),
                Some("best") => ("Best", "best"),
                _ => ("Best", "best"), // Default to "best" instead of "MP4"
            };

            // Build callback data: for mp4 always use dl:mp4:quality:url_id format
            let callback = format!("dl:mp4:{}:{}", quality_for_callback, url_id);

            (format!("📥 Download ({})", quality_display), callback)
        }
        "mp3" => (format!("📥 Download ({})", mp3_label), format!("dl:mp3:{}", url_id)),
        "photo" => ("📷 Download photo".to_string(), format!("dl:photo:{}", url_id)),
        "mp4+mp3" => ("📥 Download (MP4 + MP3)".to_string(), format!("dl:mp4+mp3:{}", url_id)),
        "srt" => ("📥 Download (SRT)".to_string(), format!("dl:srt:{}", url_id)),
        "txt" => ("📥 Download (TXT)".to_string(), format!("dl:txt:{}", url_id)),
        _ => (format!("📥 Download ({})", mp3_label), format!("dl:mp3:{}", url_id)),
    };

    let mut rows = vec![vec![crate::telegram::cb(button_text, callback_data)]];

    if default_format == "mp4" || default_format == "mp4+mp3" {
        rows.push(vec![crate::telegram::cb(
            format!("🎵 {}", mp3_label),
            format!("dl:mp3:{}", url_id),
        )]);
    }

    // Lyrics toggle button for audio downloads
    if default_format == "mp3" || default_format == "mp4" || default_format == "mp4+mp3" {
        rows.push(vec![crate::telegram::cb("☐ 📝 Lyrics", format!("dl:tl:{}", url_id))]);
    }

    if (default_format == "mp4" || default_format == "mp4+mp3") && (is_youtube || burn_sub_lang.is_some()) {
        let label = match burn_sub_lang {
            Some(lang) => format!("🔤 Subs: {} ✓", lang),
            None => "🔤 Burn subtitles".to_string(),
        };
        rows.push(vec![crate::telegram::cb(label, format!("pv:burn_subs:{}", url_id))]);
    }

    rows.push(vec![crate::telegram::cb(
        "⚙️ Settings".to_string(),
        format!("pv:set:{}", url_id),
    )]);
    rows.push(vec![crate::telegram::cb(
        "❌ Cancel".to_string(),
        format!("pv:cancel:{}", url_id),
    )]);

    InlineKeyboardMarkup::new(rows)
}

/// Creates a keyboard for video format selection
///
/// - Large button for default format (from user settings)
/// - Small buttons for other formats (2 per row)
/// - Toggle button for Media/Document selection
/// - Large "Cancel" button at the bottom
pub fn create_video_format_keyboard(
    formats: &[VideoFormatInfo],
    default_quality: Option<&str>,
    url_id: &str,
    send_as_document: i32,
    default_format: &str,
    audio_bitrate: Option<&str>,
    is_youtube: bool,
    burn_sub_lang: Option<&str>,
) -> InlineKeyboardMarkup {
    log::debug!(
        "Creating video format keyboard (formats={}, default_quality={:?}, url_id={}, send_as_document={}, format={})",
        formats.len(),
        default_quality,
        url_id,
        send_as_document,
        default_format
    );
    let mp3_label = audio_bitrate
        .map(|bitrate| format!("MP3 {}", bitrate))
        .unwrap_or_else(|| "MP3".to_string());
    let mut buttons: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    // Find default format (from user settings)
    // Map "best" to the first (best) format in the list
    let default_format_info = if let Some(quality) = default_quality {
        if quality == "best" {
            formats.first()
        } else {
            formats
                .iter()
                .find(|f| f.quality == quality)
                .or_else(|| formats.first())
        }
    } else {
        formats.first()
    };

    // Large button for default format (MP4 only; for MP4+MP3 show all as small buttons)
    if default_format != "mp4+mp3" {
        if let Some(format_info) = default_format_info {
            let size_str = format_info
                .size_bytes
                .map(|s| {
                    if s > 1024 * 1024 {
                        format!("{:.1} MB", s as f64 / (1024.0 * 1024.0))
                    } else if s > 1024 {
                        format!("{:.1} KB", s as f64 / 1024.0)
                    } else {
                        format!("{} B", s)
                    }
                })
                .unwrap_or_else(|| "?".to_string());

            buttons.push(vec![crate::telegram::cb(
                format!("📥 {} ({})", format_info.quality, size_str),
                format!("dl:{}:{}:{}", default_format, format_info.quality, url_id),
            )]);
        }
    }

    // Small buttons for formats (2 per row)
    // For MP4+MP3 show ALL formats; for MP4 exclude default and show max 4
    let mut row = Vec::new();
    let default_index = if default_format == "mp4+mp3" {
        usize::MAX // For MP4+MP3 don't exclude default, show all
    } else {
        default_format_info
            .and_then(|df| formats.iter().position(|f| f.quality == df.quality))
            .unwrap_or(usize::MAX) // If default not found, skip all
    };

    let mut added_count = 0;
    // For MP4+MP3 show all formats; for MP4 show at most 4 additional formats
    let max_formats = if default_format == "mp4+mp3" {
        formats.len() // Show all formats for MP4+MP3
    } else {
        4 // For MP4 show max 4 additional formats
    };

    for (idx, format_info) in formats.iter().enumerate() {
        // For MP4, skip the default; for MP4+MP3 show all
        if default_format != "mp4+mp3" && idx == default_index {
            continue; // Skip default format only for MP4
        }

        if added_count >= max_formats {
            break;
        }

        let size_str = format_info
            .size_bytes
            .map(|s| {
                if s > 1024 * 1024 {
                    format!("{:.1}MB", s as f64 / (1024.0 * 1024.0))
                } else if s > 1024 {
                    format!("{:.1}KB", s as f64 / 1024.0)
                } else {
                    format!("{}B", s)
                }
            })
            .unwrap_or_else(|| "?".to_string());

        row.push(crate::telegram::cb(
            format!("{} {}", format_info.quality, size_str),
            format!("dl:{}:{}:{}", default_format, format_info.quality, url_id),
        ));
        added_count += 1;

        if row.len() == 2 {
            buttons.push(row);
            row = Vec::new();
        }
    }

    // Add remaining buttons if any
    if !row.is_empty() {
        buttons.push(row);
    }

    buttons.push(vec![crate::telegram::cb(
        format!("🎵 {}", mp3_label),
        format!("dl:mp3:{}", url_id),
    )]);

    // MP3 toggle button (on = mp4+mp3 mode, off = mp4 only)
    let mp3_on = default_format == "mp4+mp3";
    buttons.push(vec![crate::telegram::cb(
        if mp3_on {
            "☑ + 🎵 MP3".to_string()
        } else {
            "☐ 🎵 MP3".to_string()
        },
        format!("dl:tm:{}", url_id),
    )]);

    // Lyrics toggle button (for MP3 downloads)
    buttons.push(vec![crate::telegram::cb("☐ 📝 Lyrics", format!("dl:tl:{}", url_id))]);

    // Toggle button for send type (Media/Document)
    buttons.push(vec![crate::telegram::cb(
        if send_as_document == 0 {
            "📹 Send as: Media ✓"
        } else {
            "📄 Send as: Document ✓"
        }
        .to_string(),
        format!("video_send_type:toggle:{}", url_id),
    )]);

    // Burn subtitles button for YouTube videos (or when a language is already selected)
    if is_youtube || burn_sub_lang.is_some() {
        let label = match burn_sub_lang {
            Some(lang) => format!("🔤 Subs: {} ✓", lang),
            None => "🔤 Burn subtitles".to_string(),
        };
        buttons.push(vec![crate::telegram::cb(label, format!("pv:burn_subs:{}", url_id))]);
    }

    // Settings button
    buttons.push(vec![crate::telegram::cb(
        "⚙️ Settings".to_string(),
        format!("pv:set:{}", url_id),
    )]);

    // Large Cancel button at the bottom
    buttons.push(vec![crate::telegram::cb(
        "❌ Cancel".to_string(),
        format!("pv:cancel:{}", url_id),
    )]);

    InlineKeyboardMarkup::new(buttons)
}

/// Number emojis for carousel item buttons (1-indexed, index 0 = "1️⃣")
const NUM_EMOJI: [&str; 10] = ["1️⃣", "2️⃣", "3️⃣", "4️⃣", "5️⃣", "6️⃣", "7️⃣", "8️⃣", "9️⃣", "🔟"];

/// Creates an inline keyboard for Instagram carousel item selection.
///
/// Each item gets a toggle button; selected items are marked with ✅, deselected with ⬜.
/// Selection state is encoded as a bitmask in the callback data (stateless — no server state needed).
///
/// # Layout
/// ```text
/// [1️⃣ ✅] [2️⃣ ✅] [3️⃣ ⬜] [4️⃣ ✅] [5️⃣ ✅]
/// [✅ All] [❌ Reset]
/// [📷 Download selected (4)]
/// [⚙️ Settings]
/// [❌ Cancel]
/// ```
pub fn create_carousel_keyboard(carousel_count: u8, mask: u32, url_id: &str) -> InlineKeyboardMarkup {
    let count = carousel_count as usize;
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    // Item toggle buttons in rows of 5
    let mut current_row: Vec<InlineKeyboardButton> = Vec::new();
    for i in 0..count {
        let selected = mask & (1 << i) != 0;
        let emoji = NUM_EMOJI.get(i).unwrap_or(&"▪️");
        let check = if selected { "✅" } else { "⬜" };
        let label = format!("{} {}", emoji, check);
        // Toggle: flip this bit in the mask
        let new_mask = mask ^ (1 << i);
        let callback = format!("ct:{}:{}:{}", i, url_id, new_mask);
        current_row.push(crate::telegram::cb(label, callback));
        if current_row.len() == 5 || i == count - 1 {
            rows.push(std::mem::take(&mut current_row));
        }
    }

    // Select all / Clear all
    let full_mask = (1u32 << count) - 1;
    rows.push(vec![
        crate::telegram::cb("✅ All".to_string(), format!("ct:all:{}:{}", url_id, full_mask)),
        crate::telegram::cb("❌ Reset".to_string(), format!("ct:all:{}:0", url_id)),
    ]);

    // Download button with count of selected items
    let selected_count = (0..count).filter(|i| mask & (1 << i) != 0).count();
    let dl_label = format!("📷 Download selected ({})", selected_count);
    rows.push(vec![crate::telegram::cb(
        dl_label,
        format!("dl:photo:{}:{}", url_id, mask),
    )]);

    // Settings button
    rows.push(vec![crate::telegram::cb(
        "⚙️ Settings".to_string(),
        format!("pv:set:{}", url_id),
    )]);

    // Cancel button
    rows.push(vec![crate::telegram::cb(
        "❌ Cancel".to_string(),
        format!("pv:cancel:{}", url_id),
    )]);

    InlineKeyboardMarkup::new(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== keyboard_stats tests ====================

    #[test]
    fn test_keyboard_stats_empty() {
        let keyboard = InlineKeyboardMarkup::new(Vec::<Vec<InlineKeyboardButton>>::new());
        assert_eq!(keyboard_stats(&keyboard), (0, 0));
    }

    #[test]
    fn test_keyboard_stats_single_row() {
        let keyboard = InlineKeyboardMarkup::new(vec![vec![
            InlineKeyboardButton::callback("Button 1", "data1"),
            InlineKeyboardButton::callback("Button 2", "data2"),
        ]]);
        assert_eq!(keyboard_stats(&keyboard), (1, 2));
    }

    #[test]
    fn test_keyboard_stats_multiple_rows() {
        let keyboard = InlineKeyboardMarkup::new(vec![
            vec![InlineKeyboardButton::callback("A", "a")],
            vec![
                InlineKeyboardButton::callback("B", "b"),
                InlineKeyboardButton::callback("C", "c"),
            ],
            vec![
                InlineKeyboardButton::callback("D", "d"),
                InlineKeyboardButton::callback("E", "e"),
                InlineKeyboardButton::callback("F", "f"),
            ],
        ]);
        assert_eq!(keyboard_stats(&keyboard), (3, 6));
    }

    // ==================== burn_sub_lang button tests ====================

    /// Helper: find a button by label substring in the keyboard
    fn find_button_text(keyboard: &InlineKeyboardMarkup, needle: &str) -> Option<String> {
        for row in &keyboard.inline_keyboard {
            for btn in row {
                if btn.text.contains(needle) {
                    return Some(btn.text.clone());
                }
            }
        }
        None
    }

    /// Helper: check if any button's callback data contains the given substring
    fn has_callback_containing(keyboard: &InlineKeyboardMarkup, needle: &str) -> bool {
        keyboard.inline_keyboard.iter().any(|row| {
            row.iter().any(|btn| {
                matches!(
                    &btn.kind,
                    teloxide::types::InlineKeyboardButtonKind::CallbackData(d) if d.contains(needle)
                )
            })
        })
    }

    #[test]
    fn test_fallback_keyboard_youtube_no_lang_shows_burn_subs() {
        let kb = create_fallback_keyboard("mp4", Some("1080p"), "test_id", Some("320k"), true, None);
        assert_eq!(
            find_button_text(&kb, "Burn subtitles"),
            Some("🔤 Burn subtitles".to_string())
        );
    }

    #[test]
    fn test_fallback_keyboard_youtube_with_lang_shows_subs_lang() {
        let kb = create_fallback_keyboard("mp4", Some("1080p"), "test_id", Some("320k"), true, Some("en"));
        assert_eq!(find_button_text(&kb, "Subs:"), Some("🔤 Subs: en ✓".to_string()));
        // Should NOT also show "Burn subtitles"
        assert_eq!(find_button_text(&kb, "Burn subtitles"), None);
    }

    #[test]
    fn test_fallback_keyboard_not_youtube_no_burn_subs_button() {
        let kb = create_fallback_keyboard("mp4", Some("1080p"), "test_id", Some("320k"), false, None);
        assert_eq!(find_button_text(&kb, "Burn subtitles"), None);
        assert_eq!(find_button_text(&kb, "Subs:"), None);
    }

    #[test]
    fn test_fallback_keyboard_mp3_no_burn_subs_button() {
        // Burn subs only makes sense for video formats
        let kb = create_fallback_keyboard("mp3", None, "test_id", Some("320k"), true, None);
        assert_eq!(find_button_text(&kb, "Burn subtitles"), None);
    }

    #[test]
    fn test_video_format_keyboard_youtube_no_lang_shows_burn_subs() {
        let formats = vec![VideoFormatInfo {
            quality: "1080p".to_string(),
            size_bytes: Some(100_000_000),
            resolution: Some("1920x1080".to_string()),
        }];
        let kb = create_video_format_keyboard(&formats, Some("1080p"), "test_id", 0, "mp4", Some("320k"), true, None);
        assert_eq!(
            find_button_text(&kb, "Burn subtitles"),
            Some("🔤 Burn subtitles".to_string())
        );
    }

    #[test]
    fn test_video_format_keyboard_youtube_with_lang_shows_subs_lang() {
        let formats = vec![VideoFormatInfo {
            quality: "1080p".to_string(),
            size_bytes: Some(100_000_000),
            resolution: Some("1920x1080".to_string()),
        }];
        let kb = create_video_format_keyboard(
            &formats,
            Some("1080p"),
            "test_id",
            0,
            "mp4",
            Some("320k"),
            true,
            Some("ru"),
        );
        assert_eq!(find_button_text(&kb, "Subs:"), Some("🔤 Subs: ru ✓".to_string()));
        assert_eq!(find_button_text(&kb, "Burn subtitles"), None);
    }

    #[test]
    fn test_video_format_keyboard_not_youtube_no_burn_subs() {
        let formats = vec![VideoFormatInfo {
            quality: "720p".to_string(),
            size_bytes: Some(50_000_000),
            resolution: Some("1280x720".to_string()),
        }];
        let kb = create_video_format_keyboard(&formats, Some("720p"), "test_id", 0, "mp4", Some("320k"), false, None);
        assert_eq!(find_button_text(&kb, "Burn subtitles"), None);
        assert_eq!(find_button_text(&kb, "Subs:"), None);
    }

    #[test]
    fn test_burn_subs_callback_data_format() {
        let kb = create_fallback_keyboard("mp4", Some("1080p"), "abc123", Some("320k"), true, Some("de"));
        assert!(has_callback_containing(&kb, "pv:burn_subs:abc123"));
    }
}
