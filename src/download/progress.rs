use crate::core::utils::pluralize_seconds;
use crate::telegram::Bot;
use regex::Regex;
use teloxide::prelude::*;
use teloxide::types::MessageId;

/// Состояние загрузки файла для отображения прогресса пользователю.
///
/// Используется для отслеживания различных этапов процесса загрузки и отправки файла.
#[derive(Debug, Clone)]
pub enum DownloadStatus {
    /// Начало загрузки
    Starting {
        /// Название файла/трека
        title: String,
        /// Формат файла для выбора эмодзи: "mp3", "mp4", "srt", "txt" (опционально)
        file_format: Option<String>,
    },
    /// Процесс загрузки с прогресс-баром
    Downloading {
        /// Название файла/трека
        title: String,
        /// Прогресс загрузки в процентах (0-100)
        progress: u8,
        /// Скорость загрузки в MB/s (опционально)
        speed_mbs: Option<f64>,
        /// Примерное время до завершения в секундах (опционально)
        eta_seconds: Option<u64>,
        /// Текущий размер в байтах (опционально)
        current_size: Option<u64>,
        /// Общий размер в байтах (опционально)
        total_size: Option<u64>,
        /// Формат файла для выбора эмодзи: "mp3", "mp4", "srt", "txt" (опционально)
        file_format: Option<String>,
    },
    /// Отправка файла на сервер Telegram
    Uploading {
        /// Название файла/трека
        title: String,
        /// Количество точек для анимации (0-3)
        dots: u8,
        /// Примерный прогресс отправки в процентах (0-100, опционально)
        progress: Option<u8>,
        /// Скорость отправки в MB/s (опционально)
        speed_mbs: Option<f64>,
        /// Примерное время до завершения в секундах (опционально)
        eta_seconds: Option<u64>,
        /// Текущий размер в байтах (опционально)
        current_size: Option<u64>,
        /// Общий размер в байтах (опционально)
        total_size: Option<u64>,
        /// Формат файла для выбора эмодзи: "mp3", "mp4", "srt", "txt" (опционально)
        file_format: Option<String>,
    },
    /// Успешная загрузка с информацией о времени
    Success {
        /// Название файла/трека
        title: String,
        /// Затраченное время в секундах
        elapsed_secs: u64,
        /// Формат файла для выбора эмодзи: "mp3", "mp4", "srt", "txt" (опционально)
        file_format: Option<String>,
    },
    /// Финальное состояние (только название, без дополнительной информации)
    Completed {
        /// Название файла/трека
        title: String,
        /// Формат файла для выбора эмодзи: "mp3", "mp4", "srt", "txt" (опционально)
        file_format: Option<String>,
    },
    /// Ошибка при загрузке
    Error {
        /// Название файла/трека
        title: String,
        /// Описание ошибки
        error: String,
        /// Формат файла для выбора эмодзи: "mp3", "mp4", "srt", "txt" (опционально)
        file_format: Option<String>,
    },
}

impl DownloadStatus {
    /// Возвращает эмодзи в зависимости от формата файла
    ///
    /// # Arguments
    ///
    /// * `file_format` - Формат файла: "mp3", "mp4", "srt", "txt" или None
    ///
    /// # Returns
    ///
    /// Эмодзи для соответствующего типа файла или 🎵 по умолчанию
    fn get_emoji(file_format: Option<&String>) -> &'static str {
        match file_format {
            Some(format) => match format.as_str() {
                "mp4" | "mp4+mp3" => "🎬",
                "srt" => "📝",
                "txt" => "📄",
                _ => "🎵", // mp3 и другие форматы по умолчанию
            },
            None => "🎵", // По умолчанию нота для обратной совместимости
        }
    }

    /// Генерирует форматированный текст сообщения для текущего состояния.
    ///
    /// Форматирует сообщение в соответствии с MarkdownV2 синтаксисом Telegram,
    /// включая прогресс-бар для состояния загрузки и экранирование специальных символов.
    ///
    /// # Returns
    ///
    /// Строка с форматированным сообщением о статусе загрузки.
    ///
    /// # Example
    ///
    /// ```
    /// use doradura::download::progress::DownloadStatus;
    ///
    /// let status = DownloadStatus::Downloading {
    ///     title: "Test Song".to_string(),
    ///     progress: 50,
    ///     speed_mbs: None,
    ///     eta_seconds: None,
    ///     current_size: None,
    ///     total_size: None,
    ///     file_format: Some("mp3".to_string()),
    /// };
    /// let message = status.to_message();
    /// ```
    pub fn to_message(&self) -> String {
        match self {
            DownloadStatus::Starting { title, file_format } => {
                let escaped = escape_markdown(title);
                let emoji = Self::get_emoji(file_format.as_ref());
                let mut s = String::with_capacity(escaped.len() + 50);
                s.push_str(emoji);
                s.push_str(" *");
                s.push_str(&escaped);
                s.push_str("*\n\n⏳ Начинаю скачивание\\.\\.\\.");
                s
            }
            DownloadStatus::Downloading {
                title,
                progress,
                speed_mbs,
                eta_seconds,
                current_size,
                total_size,
                file_format,
            } => {
                let escaped = escape_markdown(title);
                let emoji = Self::get_emoji(file_format.as_ref());
                let bar = create_progress_bar(*progress);
                let mut s = String::with_capacity(escaped.len() + bar.len() + 200);
                s.push_str(emoji);
                s.push_str(" *");
                s.push_str(&escaped);
                s.push_str("*\n\n📥 Скачиваю: ");
                s.push_str(&progress.to_string());
                s.push_str("%\n");
                s.push_str(&bar);

                // Добавляем скорость, ETA и размер если доступны
                if let Some(speed) = speed_mbs {
                    s.push_str("\n\n⚡ Скорость: ");
                    // Экранируем точку в числе с плавающей точкой
                    s.push_str(&format!("{:.1} MB/s", speed).replace('.', "\\."));
                }

                if let Some(eta) = eta_seconds {
                    let minutes = eta / 60;
                    let seconds = eta % 60;
                    s.push_str("\n⏱️ Осталось: ");
                    if minutes > 0 {
                        // Экранируем числа и текст для MarkdownV2
                        let escaped_min = escape_markdown(&minutes.to_string());
                        let escaped_sec = escape_markdown(&seconds.to_string());
                        s.push_str(&format!("{} мин {} сек", escaped_min, escaped_sec));
                    } else {
                        let escaped_sec = escape_markdown(&seconds.to_string());
                        s.push_str(&format!("{} сек", escaped_sec));
                    }
                }

                if let (Some(current), Some(total)) = (current_size, total_size) {
                    let current_mb = *current as f64 / (1024.0 * 1024.0);
                    let total_mb = *total as f64 / (1024.0 * 1024.0);
                    s.push_str("\n📦 Размер: ");
                    // Экранируем точки в числах с плавающей точкой
                    s.push_str(&format!("{:.1} / {:.1} MB", current_mb, total_mb).replace('.', "\\."));
                }

                s
            }
            DownloadStatus::Uploading {
                title,
                dots,
                progress,
                speed_mbs,
                eta_seconds,
                current_size,
                total_size,
                file_format,
            } => {
                let escaped = escape_markdown(title);
                let emoji = Self::get_emoji(file_format.as_ref());
                let mut s = String::with_capacity(escaped.len() + 2000);
                s.push_str(emoji);
                s.push_str(" *");
                s.push_str(&escaped);
                s.push_str("*\n\n📤 Отправка файла");

                // Если есть прогресс - показываем прогресс-бар
                if let Some(p) = *progress {
                    let bar = create_progress_bar(p);
                    s.push_str(": ");
                    s.push_str(&p.to_string());
                    s.push_str("%\n");
                    s.push_str(&bar);
                } else {
                    // Иначе показываем анимацию точек
                    let dots_count = (*dots % 4) as usize;
                    let dots_str = if dots_count == 0 {
                        String::new()
                    } else {
                        "\\.".repeat(dots_count)
                    };
                    s.push_str(&dots_str);
                }

                if let Some(speed) = speed_mbs {
                    s.push_str("\n\n⚡ Скорость: ");
                    s.push_str(&format!("{:.1} MB/s", speed).replace('.', "\\."));
                }

                // Добавляем ETA если доступно
                if let Some(eta) = eta_seconds {
                    let minutes = eta / 60;
                    let seconds = eta % 60;
                    s.push_str("\n⏱️ Осталось: ");
                    if minutes > 0 {
                        let escaped_min = escape_markdown(&minutes.to_string());
                        let escaped_sec = escape_markdown(&seconds.to_string());
                        s.push_str(&format!("{} мин {} сек", escaped_min, escaped_sec));
                    } else {
                        let escaped_sec = escape_markdown(&seconds.to_string());
                        s.push_str(&format!("{} сек", escaped_sec));
                    }
                }

                // Добавляем размер если доступно
                if let (Some(current), Some(total)) = (current_size, total_size) {
                    let current_mb = *current as f64 / (1024.0 * 1024.0);
                    let total_mb = *total as f64 / (1024.0 * 1024.0);
                    s.push_str("\n📦 Размер: ");
                    s.push_str(&format!("{:.1} / {:.1} MB", current_mb, total_mb).replace('.', "\\."));
                }

                s
            }
            DownloadStatus::Success {
                title,
                elapsed_secs,
                file_format,
            } => {
                let escaped = escape_markdown(title);
                let emoji = Self::get_emoji(file_format.as_ref());
                let elapsed_str = elapsed_secs.to_string();
                let plural = pluralize_seconds(*elapsed_secs);
                let mut s = String::with_capacity(escaped.len() + elapsed_str.len() + plural.len() + 50);
                s.push_str(emoji);
                s.push_str(" *");
                s.push_str(&escaped);
                s.push_str("*\n\n✅ Скачано успешно за ");
                s.push_str(&elapsed_str);
                s.push(' ');
                s.push_str(plural);
                s.push_str("\\!");
                s
            }
            DownloadStatus::Completed { title, file_format } => {
                let escaped = escape_markdown(title);
                let emoji = Self::get_emoji(file_format.as_ref());
                let mut s = String::with_capacity(escaped.len() + 10);
                s.push_str(emoji);
                s.push_str(" *");
                s.push_str(&escaped);
                s.push('*');
                s
            }
            DownloadStatus::Error {
                title,
                error,
                file_format,
            } => {
                let escaped_title = escape_markdown(title);
                let escaped_error = escape_markdown(error);
                let emoji = Self::get_emoji(file_format.as_ref());
                let mut s = String::with_capacity(escaped_title.len() + escaped_error.len() + 30);
                s.push_str(emoji);
                s.push_str(" *");
                s.push_str(&escaped_title);
                s.push_str("*\n\n❌ Ошибка: ");
                s.push_str(&escaped_error);
                s
            }
        }
    }
}

/// Создает визуальный прогресс-бар
fn create_progress_bar(progress: u8) -> String {
    let progress = progress.min(100);
    let filled = (progress / 10) as usize;
    let empty = 10 - filled;

    let filled_blocks = "█".repeat(filled);
    let empty_blocks = "░".repeat(empty);

    format!("[{}{}]", filled_blocks, empty_blocks)
}

/// Извлекает время ожидания из ошибки Telegram API (rate limiting)
///
/// Парсит строку ошибки вида "Retry after Xs" и возвращает количество секунд
fn extract_retry_after(error_str: &str) -> Option<u64> {
    // Пробуем найти паттерн "Retry after Xs" или "retry_after: X"
    let re = Regex::new(r"(?i)retry\s+after\s+(\d+)\s*s").ok()?;
    if let Some(caps) = re.captures(error_str) {
        if let Some(seconds_str) = caps.get(1) {
            return seconds_str.as_str().parse::<u64>().ok();
        }
    }

    // Альтернативный паттерн: "retry_after: X"
    let re2 = Regex::new(r"(?i)retry_after[:\s]+(\d+)").ok()?;
    if let Some(caps) = re2.captures(error_str) {
        if let Some(seconds_str) = caps.get(1) {
            return seconds_str.as_str().parse::<u64>().ok();
        }
    }

    None
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

/// Структура для управления сообщением с прогрессом загрузки.
///
/// Отслеживает ID сообщения с прогрессом и позволяет обновлять его по мере выполнения загрузки.
pub struct ProgressMessage {
    /// ID чата пользователя
    pub chat_id: ChatId,
    /// ID сообщения с прогрессом (None если еще не отправлено)
    pub message_id: Option<MessageId>,
}

impl ProgressMessage {
    /// Создает новое сообщение прогресса для указанного чата.
    ///
    /// # Arguments
    ///
    /// * `chat_id` - ID чата пользователя
    ///
    /// # Returns
    ///
    /// Новый экземпляр `ProgressMessage` без отправленного сообщения.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use teloxide::types::ChatId;
    /// use doradura::download::progress::ProgressMessage;
    ///
    /// let mut progress = ProgressMessage::new(ChatId(123456789));
    /// ```
    pub fn new(chat_id: ChatId) -> Self {
        Self {
            chat_id,
            message_id: None,
        }
    }

    /// Отправляет или обновляет сообщение с прогрессом загрузки.
    ///
    /// Если сообщение еще не было отправлено, создает новое. Если уже существует,
    /// редактирует существующее сообщение. При ошибке редактирования отправляет новое сообщение.
    ///
    /// # Arguments
    ///
    /// * `bot` - Экземпляр Telegram бота
    /// * `status` - Текущее состояние загрузки
    ///
    /// # Returns
    ///
    /// Возвращает `ResponseResult<()>` или ошибку при отправке/редактировании сообщения.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use teloxide::prelude::*;
    /// use doradura::download::progress::{ProgressMessage, DownloadStatus};
    ///
    /// # async fn example(bot: Bot, chat_id: ChatId) -> ResponseResult<()> {
    /// let mut progress = ProgressMessage::new(chat_id);
    /// progress.update(&bot, DownloadStatus::Starting {
    ///     title: "Test Song".to_string(),
    ///     file_format: Some("mp3".to_string())
    /// }).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn update(&mut self, bot: &Bot, status: DownloadStatus) -> ResponseResult<()> {
        let text = status.to_message();

        if let Some(msg_id) = self.message_id {
            // Обновляем существующее сообщение
            match bot
                .edit_message_text(self.chat_id, msg_id, text.clone())
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .await
            {
                Ok(_) => Ok(()),
                Err(e) => {
                    let error_str = e.to_string();
                    // Если сообщение не изменилось - это нормально, не нужно отправлять новое
                    if error_str.contains("message is not modified") {
                        // Это нормальная ситуация - сообщение уже содержит этот контент
                        // Не логируем как ошибку и не отправляем новое сообщение
                        return Ok(());
                    }

                    // Проверяем rate limiting
                    if let Some(retry_after_secs) = extract_retry_after(&error_str) {
                        log::warn!(
                            "Rate limit hit when editing message: Retry after {}s. Waiting...",
                            retry_after_secs
                        );
                        // Ждем указанное время + небольшая задержка для надежности
                        tokio::time::sleep(tokio::time::Duration::from_secs(retry_after_secs + 1)).await;
                        // Пробуем еще раз отредактировать
                        match bot
                            .edit_message_text(self.chat_id, msg_id, text.clone())
                            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                            .await
                        {
                            Ok(_) => return Ok(()),
                            Err(e2) => {
                                let error_str2 = e2.to_string();
                                // Если снова rate limit или другая ошибка - отправляем новое сообщение
                                if error_str2.contains("message is not modified") {
                                    return Ok(());
                                }
                                log::warn!(
                                    "Still failed to edit message after rate limit wait: {}. Trying to send new one.",
                                    e2
                                );
                            }
                        }
                    } else {
                        log::warn!("Failed to edit message: {}. Trying to send new one.", e);
                    }

                    // Если не удалось отредактировать по другой причине, отправляем новое
                    let msg = bot
                        .send_message(self.chat_id, text)
                        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                        .await?;
                    self.message_id = Some(msg.id);
                    Ok(())
                }
            }
        } else {
            // Отправляем новое сообщение
            let msg = bot
                .send_message(self.chat_id, text)
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .await?;
            self.message_id = Some(msg.id);
            Ok(())
        }
    }

    /// Очищает сообщение (оставляет только название) после указанной задержки.
    ///
    /// Полезно для очистки деталей прогресса после успешной загрузки, оставляя только название файла.
    ///
    /// # Arguments
    ///
    /// * `bot` - Экземпляр Telegram бота
    /// * `delay_secs` - Задержка в секундах перед очисткой
    /// * `title` - Название файла для финального сообщения
    ///
    /// # Returns
    ///
    /// Возвращает `ResponseResult<()>` или ошибку при обновлении сообщения.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use teloxide::prelude::*;
    /// use doradura::download::progress::ProgressMessage;
    ///
    /// # async fn example(bot: Bot, mut progress: ProgressMessage) -> ResponseResult<()> {
    /// // Очистить сообщение через 10 секунд
    /// progress.clear_after(&bot, 10, "Test Song".to_string(), Some("mp3".to_string())).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn clear_after(
        &mut self,
        bot: &Bot,
        delay_secs: u64,
        title: String,
        file_format: Option<String>,
    ) -> ResponseResult<()> {
        if self.message_id.is_some() {
            tokio::time::sleep(tokio::time::Duration::from_secs(delay_secs)).await;
            self.update(
                bot,
                DownloadStatus::Completed {
                    title: title.clone(),
                    file_format,
                },
            )
            .await?;
            log::info!(
                "Cleared progress message for chat {} after {} seconds",
                self.chat_id,
                delay_secs
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_bar() {
        assert_eq!(create_progress_bar(0), "[░░░░░░░░░░]");
        assert_eq!(create_progress_bar(50), "[█████░░░░░]");
        assert_eq!(create_progress_bar(100), "[██████████]");
    }

    #[test]
    fn test_escape_markdown() {
        assert_eq!(escape_markdown("Hello World"), "Hello World");
        assert_eq!(escape_markdown("Test_file.mp3"), "Test\\_file\\.mp3");
        assert_eq!(escape_markdown("Song [2024]"), "Song \\[2024\\]");
    }
}
