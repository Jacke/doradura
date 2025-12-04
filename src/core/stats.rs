use crate::storage::db::{self, DbPool};
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::RequestError;

/// Форматирует размер в читаемый формат
fn format_size(bytes: i64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

/// Безопасно обрезает строку до указанной длины символов (не байт!)
/// Возвращает обрезанную строку с добавлением "..." если была обрезка
fn truncate_string_safe(text: &str, max_len: usize) -> String {
    if text.is_empty() {
        return String::new();
    }

    let char_count = text.chars().count();
    if char_count <= max_len {
        return text.to_string();
    }

    // Безопасно обрезаем до max_len - 3 символов, чтобы поместить "..."
    let truncate_len = max_len.saturating_sub(3);
    let mut result = String::with_capacity(truncate_len + 3);
    for (idx, ch) in text.chars().enumerate() {
        if idx >= truncate_len {
            break;
        }
        result.push(ch);
    }
    result.push_str("...");
    result
}

/// Создает ASCII график активности
fn create_activity_chart(activity_by_day: &[(String, i64)]) -> String {
    if activity_by_day.is_empty() {
        return "Нет данных".to_string();
    }

    let max_count = activity_by_day
        .iter()
        .map(|(_, count)| *count)
        .max()
        .unwrap_or(1);
    let max_bars = 10;

    let mut chart = String::new();
    for (day, count) in activity_by_day.iter().take(7) {
        let bars = if max_count > 0 {
            (count * max_bars as i64 / max_count) as usize
        } else {
            0
        };
        let bar_string = "█".repeat(bars) + &"░".repeat(max_bars - bars);

        // Форматируем дату (из "YYYY-MM-DD" в короткий формат)
        let day_short = if day.len() >= 10 {
            let parts: Vec<&str> = day.split('-').collect();
            if parts.len() >= 3 {
                format!("{}.{}", parts[2], parts[1])
            } else {
                day.clone()
            }
        } else {
            day.clone()
        };

        chart.push_str(&format!("{}: {} {}\n", day_short, bar_string, count));
    }
    chart
}

/// Показывает статистику пользователя
pub async fn show_user_stats(
    bot: &Bot,
    chat_id: ChatId,
    db_pool: Arc<DbPool>,
) -> ResponseResult<Message> {
    let conn = db::get_connection(&db_pool).map_err(|e| {
        RequestError::from(std::sync::Arc::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            e.to_string(),
        )))
    })?;

    let stats = match db::get_user_stats(&conn, chat_id.0) {
        Ok(stats) => stats,
        Err(e) => {
            log::error!("Failed to get user stats: {}", e);
            return bot
                .send_message(
                    chat_id,
                    "У меня не получилось загрузить статистику 😢 Попробуй позже\\.",
                )
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .await;
        }
    };

    let mut text = "📊 *Твоя статистика*\n\n".to_string();

    text.push_str(&format!("🎵 Всего загрузок: {}\n", stats.total_downloads));
    text.push_str(&format!("📅 Дней активности: {}\n", stats.active_days));
    text.push_str(&format!(
        "💾 Общий размер: {}\n\n",
        format_size(stats.total_size)
    ));

    if !stats.top_artists.is_empty() {
        text.push_str("🏆 *Топ исполнителей:*\n");
        for (idx, (artist, count)) in stats.top_artists.iter().enumerate() {
            text.push_str(&format!(
                "{}. {} \\- {} треков\n",
                idx + 1,
                escape_markdown(artist),
                count
            ));
        }
        text.push_str("\n");
    }

    if !stats.top_formats.is_empty() {
        text.push_str("📦 *Форматы:*\n");
        for (format, count) in stats.top_formats.iter() {
            let format_emoji = match format.as_str() {
                "mp3" => "🎵",
                "mp4" => "🎬",
                "srt" => "📝",
                "txt" => "📄",
                _ => "📦",
            };
            text.push_str(&format!(
                "{} {}: {}\n",
                format_emoji,
                format.to_uppercase(),
                count
            ));
        }
        text.push_str("\n");
    }

    if !stats.activity_by_day.is_empty() {
        text.push_str("📈 *Активность \\(последние 7 дней\\):*\n");
        text.push_str("```\n");
        text.push_str(&create_activity_chart(&stats.activity_by_day));
        text.push_str("```\n");
    }

    if stats.total_downloads == 0 {
        text = "📊 *Твоя статистика*\n\nУ тебя пока нет загрузок\\. Отправь мне ссылку на трек или видео\\!".to_string();
    }

    bot.send_message(chat_id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .await
}

/// Показывает глобальную статистику бота
pub async fn show_global_stats(
    bot: &Bot,
    chat_id: ChatId,
    db_pool: Arc<DbPool>,
) -> ResponseResult<Message> {
    let conn = db::get_connection(&db_pool).map_err(|e| {
        RequestError::from(std::sync::Arc::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            e.to_string(),
        )))
    })?;

    let stats = match db::get_global_stats(&conn) {
        Ok(stats) => stats,
        Err(e) => {
            log::error!("Failed to get global stats: {}", e);
            return bot
                .send_message(
                    chat_id,
                    "У меня не получилось загрузить статистику 😢 Попробуй позже\\.",
                )
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .await;
        }
    };

    let mut text = "🌍 *Глобальная статистика*\n\n".to_string();

    text.push_str(&format!("👥 Всего пользователей: {}\n", stats.total_users));
    text.push_str(&format!("📥 Всего загрузок: {}\n\n", stats.total_downloads));

    if !stats.top_tracks.is_empty() {
        text.push_str("🔥 *Топ\\-10 треков:*\n");
        for (idx, (title, count)) in stats.top_tracks.iter().enumerate() {
            // Защита от пустых или некорректных названий
            let safe_title = if title.is_empty() {
                "(Без названия)"
            } else {
                title
            };

            let escaped_title = escape_markdown(safe_title);
            // Безопасно обрезаем длинные названия до 50 символов
            let display_title = truncate_string_safe(&escaped_title, 50);
            text.push_str(&format!(
                "{}. {} \\- {} раз\n",
                idx + 1,
                display_title,
                count
            ));
        }
        text.push_str("\n");
    }

    if !stats.top_formats.is_empty() {
        text.push_str("📦 *Статистика по форматам:*\n");
        for (format, count) in stats.top_formats.iter() {
            let format_emoji = match format.as_str() {
                "mp3" => "🎵",
                "mp4" => "🎬",
                "srt" => "📝",
                "txt" => "📄",
                _ => "📦",
            };
            text.push_str(&format!(
                "{} {}: {}\n",
                format_emoji,
                format.to_uppercase(),
                count
            ));
        }
    }

    bot.send_message(chat_id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .await
}

/// Экранирует специальные символы для MarkdownV2
///
/// В Telegram MarkdownV2 требуется экранировать следующие символы:
/// _ * [ ] ( ) ~ ` > # + - = | { } . !
///
/// Важно: обратный слеш должен экранироваться первым, чтобы избежать повторного экранирования
fn escape_markdown(text: &str) -> String {
    let mut result = String::with_capacity(text.len() * 2);

    for c in text.chars() {
        match c {
            '\\' => result.push_str("\\\\"),
            '_' => result.push_str("\\_"),
            '*' => result.push_str("\\*"),
            '[' => result.push_str("\\["),
            ']' => result.push_str("\\]"),
            '(' => result.push_str("\\("),
            ')' => result.push_str("\\)"),
            '~' => result.push_str("\\~"),
            '`' => result.push_str("\\`"),
            '>' => result.push_str("\\>"),
            '#' => result.push_str("\\#"),
            '+' => result.push_str("\\+"),
            '-' => result.push_str("\\-"),
            '=' => result.push_str("\\="),
            '|' => result.push_str("\\|"),
            '{' => result.push_str("\\{"),
            '}' => result.push_str("\\}"),
            '.' => result.push_str("\\."),
            '!' => result.push_str("\\!"),
            _ => result.push(c),
        }
    }

    result
}
