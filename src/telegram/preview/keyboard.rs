use crate::telegram::types::VideoFormatInfo;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub fn keyboard_stats(keyboard: &InlineKeyboardMarkup) -> (usize, usize) {
    let rows = keyboard.inline_keyboard.len();
    let buttons = keyboard.inline_keyboard.iter().map(|row| row.len()).sum();
    (rows, buttons)
}

/// Создает стандартную клавиатуру с кнопкой скачивания
///
/// Используется как fallback когда список форматов недоступен
///
/// # Параметры
/// - `default_format` - формат файла (mp3, mp4, srt, txt)
/// - `default_quality` - качество видео (только для mp4: "1080p", "720p", "480p", "360p", "best")
/// - `url_id` - ID URL в кэше
pub fn create_fallback_keyboard(
    default_format: &str,
    default_quality: Option<&str>,
    url_id: &str,
    audio_bitrate: Option<&str>,
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

    // Формируем текст кнопки с учетом формата и качества
    let (button_text, callback_data) = match default_format {
        "mp4" => {
            // Для видео показываем качество
            let (quality_display, quality_for_callback) = match default_quality {
                Some("1080p") => ("1080p", "1080p"),
                Some("720p") => ("720p", "720p"),
                Some("480p") => ("480p", "480p"),
                Some("360p") => ("360p", "360p"),
                Some("best") => ("Best", "best"),
                _ => ("Best", "best"), // По умолчанию используем "best" вместо "MP4"
            };

            // Формируем callback data: для mp4 всегда используем формат dl:mp4:quality:url_id
            let callback = format!("dl:mp4:{}:{}", quality_for_callback, url_id);

            (format!("📥 Скачать ({})", quality_display), callback)
        }
        "mp3" => (format!("📥 Скачать ({})", mp3_label), format!("dl:mp3:{}", url_id)),
        "photo" => ("📷 Скачать фото".to_string(), format!("dl:photo:{}", url_id)),
        "mp4+mp3" => ("📥 Скачать (MP4 + MP3)".to_string(), format!("dl:mp4+mp3:{}", url_id)),
        "srt" => ("📥 Скачать (SRT)".to_string(), format!("dl:srt:{}", url_id)),
        "txt" => ("📥 Скачать (TXT)".to_string(), format!("dl:txt:{}", url_id)),
        _ => (format!("📥 Скачать ({})", mp3_label), format!("dl:mp3:{}", url_id)),
    };

    let mut rows = vec![vec![InlineKeyboardButton::callback(button_text, callback_data)]];

    if default_format == "mp4" || default_format == "mp4+mp3" {
        rows.push(vec![InlineKeyboardButton::callback(
            format!("🎵 {}", mp3_label),
            format!("dl:mp3:{}", url_id),
        )]);
    }

    rows.push(vec![InlineKeyboardButton::callback(
        "⚙️ Настройки".to_string(),
        format!("pv:set:{}", url_id),
    )]);
    rows.push(vec![InlineKeyboardButton::callback(
        "❌ Отмена".to_string(),
        format!("pv:cancel:{}", url_id),
    )]);

    InlineKeyboardMarkup::new(rows)
}

/// Создает клавиатуру для выбора формата видео
///
/// - Большая кнопка для default формата (из настроек пользователя)
/// - Маленькие кнопки для остальных форматов (по 2 в ряд)
/// - Toggle кнопка для выбора Media/Document
/// - Большая кнопка "Отмена" внизу
pub fn create_video_format_keyboard(
    formats: &[VideoFormatInfo],
    default_quality: Option<&str>,
    url_id: &str,
    send_as_document: i32,
    default_format: &str,
    audio_bitrate: Option<&str>,
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

    // Находим default формат (из настроек пользователя)
    // Маппим "best" на первый (лучший) формат из списка
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

    // Большая кнопка для default формата (только для MP4, для MP4+MP3 показываем все как маленькие)
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

            buttons.push(vec![InlineKeyboardButton::callback(
                format!("📥 {} ({})", format_info.quality, size_str),
                format!("dl:{}:{}:{}", default_format, format_info.quality, url_id),
            )]);
        }
    }

    // Маленькие кнопки для форматов (по 2 в ряд)
    // Для MP4+MP3 показываем ВСЕ форматы, для MP4 - исключаем default и показываем максимум 4
    let mut row = Vec::new();
    let default_index = if default_format == "mp4+mp3" {
        usize::MAX // Для MP4+MP3 не исключаем default, показываем все
    } else {
        default_format_info
            .and_then(|df| formats.iter().position(|f| f.quality == df.quality))
            .unwrap_or(usize::MAX) // Если default не найден, пропускаем все
    };

    let mut added_count = 0;
    // Для MP4+MP3 показываем все форматы, для MP4 - максимум 4 дополнительных
    let max_formats = if default_format == "mp4+mp3" {
        formats.len() // Показываем все форматы для MP4+MP3
    } else {
        4 // Для MP4 показываем максимум 4 дополнительных формата
    };

    for (idx, format_info) in formats.iter().enumerate() {
        // Для MP4 пропускаем default, для MP4+MP3 показываем все
        if default_format != "mp4+mp3" && idx == default_index {
            continue; // Пропускаем default формат только для MP4
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

        row.push(InlineKeyboardButton::callback(
            format!("{} {}", format_info.quality, size_str),
            format!("dl:{}:{}:{}", default_format, format_info.quality, url_id),
        ));
        added_count += 1;

        if row.len() == 2 {
            buttons.push(row);
            row = Vec::new();
        }
    }

    // Добавляем оставшиеся кнопки если есть
    if !row.is_empty() {
        buttons.push(row);
    }

    buttons.push(vec![InlineKeyboardButton::callback(
        format!("🎵 {}", mp3_label),
        format!("dl:mp3:{}", url_id),
    )]);

    // Toggle кнопка для выбора типа отправки (Media/Document)
    buttons.push(vec![InlineKeyboardButton::callback(
        if send_as_document == 0 {
            "📹 Отправка: Media ✓"
        } else {
            "📄 Отправка: Document ✓"
        }
        .to_string(),
        format!("video_send_type:toggle:{}", url_id),
    )]);

    // Кнопка "Настройки"
    buttons.push(vec![InlineKeyboardButton::callback(
        "⚙️ Настройки".to_string(),
        format!("pv:set:{}", url_id),
    )]);

    // Большая кнопка "Отмена" внизу
    buttons.push(vec![InlineKeyboardButton::callback(
        "❌ Отмена".to_string(),
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
/// [✅ Все] [❌ Сбросить]
/// [📷 Скачать выбранные (4)]
/// [⚙️ Настройки]
/// [❌ Отмена]
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
        current_row.push(InlineKeyboardButton::callback(label, callback));
        if current_row.len() == 5 || i == count - 1 {
            rows.push(std::mem::take(&mut current_row));
        }
    }

    // Select all / Clear all
    let full_mask = (1u32 << count) - 1;
    rows.push(vec![
        InlineKeyboardButton::callback("✅ Все".to_string(), format!("ct:all:{}:{}", url_id, full_mask)),
        InlineKeyboardButton::callback("❌ Сбросить".to_string(), format!("ct:all:{}:0", url_id)),
    ]);

    // Download button with count of selected items
    let selected_count = (0..count).filter(|i| mask & (1 << i) != 0).count();
    let dl_label = format!("📷 Скачать выбранные ({})", selected_count);
    rows.push(vec![InlineKeyboardButton::callback(
        dl_label,
        format!("dl:photo:{}:{}", url_id, mask),
    )]);

    // Settings button
    rows.push(vec![InlineKeyboardButton::callback(
        "⚙️ Настройки".to_string(),
        format!("pv:set:{}", url_id),
    )]);

    // Cancel button
    rows.push(vec![InlineKeyboardButton::callback(
        "❌ Отмена".to_string(),
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
}
