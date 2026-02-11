use crate::core::{escape_markdown_v2 as escape_markdown, extract_retry_after};
use crate::i18n;
use crate::telegram::Bot;
use fluent_templates::fluent_bundle::FluentArgs;
use teloxide::prelude::*;
use teloxide::types::MessageId;
use unic_langid::LanguageIdentifier;

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
    /// let lang: unic_langid::LanguageIdentifier = "ru".parse().unwrap();
    /// let message = status.to_message(&lang);
    /// ```
    pub fn to_message(&self, lang: &LanguageIdentifier) -> String {
        match self {
            DownloadStatus::Starting { title, file_format } => {
                let escaped = escape_markdown(title);
                let emoji = Self::get_emoji(file_format.as_ref());
                let starting_text = escape_markdown(&i18n::t(lang, "progress.starting"));
                let mut s = String::with_capacity(escaped.len() + starting_text.len() + 50);
                s.push_str(emoji);
                s.push_str(" *");
                s.push_str(&escaped);
                s.push_str("*\n\n⏳ ");
                s.push_str(&starting_text);
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
                let downloading_text = escape_markdown(&i18n::t(lang, "progress.downloading"));
                let mut s = String::with_capacity(escaped.len() + bar.len() + 200);
                s.push_str(emoji);
                s.push_str(" *");
                s.push_str(&escaped);
                s.push_str("*\n\n📥 ");
                s.push_str(&downloading_text);
                s.push_str(": ");
                s.push_str(&progress.to_string());
                s.push_str("%\n");
                s.push_str(&bar);

                if let Some(speed) = speed_mbs {
                    let speed_label = escape_markdown(&i18n::t(lang, "progress.speed"));
                    s.push_str("\n\n⚡ ");
                    s.push_str(&speed_label);
                    s.push_str(": ");
                    s.push_str(&format!("{:.1} MB/s", speed).replace('.', "\\."));
                }

                if let Some(eta) = eta_seconds {
                    let minutes = eta / 60;
                    let seconds = eta % 60;
                    let eta_label = escape_markdown(&i18n::t(lang, "progress.eta"));
                    let min_label = escape_markdown(&i18n::t(lang, "progress.min"));
                    let sec_label = escape_markdown(&i18n::t(lang, "progress.sec"));
                    s.push_str("\n⏱️ ");
                    s.push_str(&eta_label);
                    s.push_str(": ");
                    if minutes > 0 {
                        let escaped_min = escape_markdown(&minutes.to_string());
                        let escaped_sec = escape_markdown(&seconds.to_string());
                        s.push_str(&escaped_min);
                        s.push(' ');
                        s.push_str(&min_label);
                        s.push(' ');
                        s.push_str(&escaped_sec);
                        s.push(' ');
                        s.push_str(&sec_label);
                    } else {
                        let escaped_sec = escape_markdown(&seconds.to_string());
                        s.push_str(&escaped_sec);
                        s.push(' ');
                        s.push_str(&sec_label);
                    }
                }

                if let (Some(current), Some(total)) = (current_size, total_size) {
                    let current_mb = *current as f64 / (1024.0 * 1024.0);
                    let total_mb = *total as f64 / (1024.0 * 1024.0);
                    let size_label = escape_markdown(&i18n::t(lang, "progress.size"));
                    s.push_str("\n📦 ");
                    s.push_str(&size_label);
                    s.push_str(": ");
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
                let uploading_text = escape_markdown(&i18n::t(lang, "progress.uploading"));
                let mut s = String::with_capacity(escaped.len() + 2000);
                s.push_str(emoji);
                s.push_str(" *");
                s.push_str(&escaped);
                s.push_str("*\n\n📤 ");
                s.push_str(&uploading_text);

                if let Some(p) = *progress {
                    let bar = create_progress_bar(p);
                    s.push_str(": ");
                    s.push_str(&p.to_string());
                    s.push_str("%\n");
                    s.push_str(&bar);
                } else {
                    let dots_count = (*dots % 4) as usize;
                    let dots_str = if dots_count == 0 {
                        String::new()
                    } else {
                        "\\.".repeat(dots_count)
                    };
                    s.push_str(&dots_str);
                }

                if let Some(speed) = speed_mbs {
                    let speed_label = escape_markdown(&i18n::t(lang, "progress.speed"));
                    s.push_str("\n\n⚡ ");
                    s.push_str(&speed_label);
                    s.push_str(": ");
                    s.push_str(&format!("{:.1} MB/s", speed).replace('.', "\\."));
                }

                if let Some(eta) = eta_seconds {
                    let minutes = eta / 60;
                    let seconds = eta % 60;
                    let eta_label = escape_markdown(&i18n::t(lang, "progress.eta"));
                    let min_label = escape_markdown(&i18n::t(lang, "progress.min"));
                    let sec_label = escape_markdown(&i18n::t(lang, "progress.sec"));
                    s.push_str("\n⏱️ ");
                    s.push_str(&eta_label);
                    s.push_str(": ");
                    if minutes > 0 {
                        let escaped_min = escape_markdown(&minutes.to_string());
                        let escaped_sec = escape_markdown(&seconds.to_string());
                        s.push_str(&escaped_min);
                        s.push(' ');
                        s.push_str(&min_label);
                        s.push(' ');
                        s.push_str(&escaped_sec);
                        s.push(' ');
                        s.push_str(&sec_label);
                    } else {
                        let escaped_sec = escape_markdown(&seconds.to_string());
                        s.push_str(&escaped_sec);
                        s.push(' ');
                        s.push_str(&sec_label);
                    }
                }

                if let (Some(current), Some(total)) = (current_size, total_size) {
                    let current_mb = *current as f64 / (1024.0 * 1024.0);
                    let total_mb = *total as f64 / (1024.0 * 1024.0);
                    let size_label = escape_markdown(&i18n::t(lang, "progress.size"));
                    s.push_str("\n📦 ");
                    s.push_str(&size_label);
                    s.push_str(": ");
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
                let mut args = FluentArgs::new();
                args.set("elapsed", *elapsed_secs as i64);
                let success_text = escape_markdown(&i18n::t_args(lang, "progress.success", &args));
                let mut s = String::with_capacity(escaped.len() + success_text.len() + 20);
                s.push_str(emoji);
                s.push_str(" *");
                s.push_str(&escaped);
                s.push_str("*\n\n✅ ");
                s.push_str(&success_text);
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
                let error_label = escape_markdown(&i18n::t(lang, "progress.error"));
                let mut s = String::with_capacity(escaped_title.len() + escaped_error.len() + error_label.len() + 30);
                s.push_str(emoji);
                s.push_str(" *");
                s.push_str(&escaped_title);
                s.push_str("*\n\n❌ ");
                s.push_str(&error_label);
                s.push_str(": ");
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

// escape_markdown and extract_retry_after are now imported from crate::core

/// Структура для управления сообщением с прогрессом загрузки.
///
/// Отслеживает ID сообщения с прогрессом и позволяет обновлять его по мере выполнения загрузки.
pub struct ProgressMessage {
    /// ID чата пользователя
    pub chat_id: ChatId,
    /// ID сообщения с прогрессом (None если еще не отправлено)
    pub message_id: Option<MessageId>,
    /// Язык пользователя для локализации прогресс-сообщений
    pub lang: LanguageIdentifier,
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
    /// use unic_langid::LanguageIdentifier;
    ///
    /// let lang: LanguageIdentifier = "ru".parse().unwrap();
    /// let mut progress = ProgressMessage::new(ChatId(123456789), lang);
    /// ```
    pub fn new(chat_id: ChatId, lang: LanguageIdentifier) -> Self {
        Self {
            chat_id,
            message_id: None,
            lang,
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
    /// ```ignore
    /// use doradura::telegram::Bot;
    /// use doradura::download::progress::{ProgressMessage, DownloadStatus};
    /// use teloxide::types::ChatId;
    ///
    /// # async fn example(bot: Bot, chat_id: ChatId) -> teloxide::RequestError {
    /// let lang: unic_langid::LanguageIdentifier = "ru".parse().unwrap();
    /// let mut progress = ProgressMessage::new(chat_id, lang);
    /// progress.update(&bot, DownloadStatus::Starting {
    ///     title: "Test Song".to_string(),
    ///     file_format: Some("mp3".to_string())
    /// }).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn update(&mut self, bot: &Bot, status: DownloadStatus) -> ResponseResult<()> {
        let text = status.to_message(&self.lang);

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
    /// ```ignore
    /// use doradura::telegram::Bot;
    /// use doradura::download::progress::ProgressMessage;
    ///
    /// # async fn example(bot: Bot, mut progress: ProgressMessage) -> teloxide::RequestError {
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

    // ==================== create_progress_bar Tests ====================

    #[test]
    fn test_progress_bar() {
        assert_eq!(create_progress_bar(0), "[░░░░░░░░░░]");
        assert_eq!(create_progress_bar(50), "[█████░░░░░]");
        assert_eq!(create_progress_bar(100), "[██████████]");
    }

    #[test]
    fn test_progress_bar_intermediate_values() {
        assert_eq!(create_progress_bar(10), "[█░░░░░░░░░]");
        assert_eq!(create_progress_bar(25), "[██░░░░░░░░]");
        assert_eq!(create_progress_bar(75), "[███████░░░]");
        assert_eq!(create_progress_bar(90), "[█████████░]");
    }

    #[test]
    fn test_progress_bar_overflow() {
        // Progress > 100 should be capped
        assert_eq!(create_progress_bar(150), "[██████████]");
        assert_eq!(create_progress_bar(255), "[██████████]");
    }

    // ==================== escape_markdown Tests ====================

    #[test]
    fn test_escape_markdown() {
        assert_eq!(escape_markdown("Hello World"), "Hello World");
        assert_eq!(escape_markdown("Test_file.mp3"), "Test\\_file\\.mp3");
        assert_eq!(escape_markdown("Song [2024]"), "Song \\[2024\\]");
    }

    #[test]
    fn test_escape_markdown_all_special() {
        let input = r"_*[]()~`>#+-=|{}.!";
        let expected = r"\_\*\[\]\(\)\~\`\>\#\+\-\=\|\{\}\.\!";
        assert_eq!(escape_markdown(input), expected);
    }

    #[test]
    fn test_escape_markdown_backslash() {
        assert_eq!(escape_markdown("a\\b"), "a\\\\b");
    }

    #[test]
    fn test_escape_markdown_empty() {
        assert_eq!(escape_markdown(""), "");
    }

    // ==================== extract_retry_after Tests ====================

    #[test]
    fn test_extract_retry_after_standard() {
        assert_eq!(extract_retry_after("Retry after 30s"), Some(30));
        assert_eq!(extract_retry_after("retry after 60s"), Some(60));
    }

    #[test]
    fn test_extract_retry_after_colon_format() {
        assert_eq!(extract_retry_after("retry_after: 45"), Some(45));
        assert_eq!(extract_retry_after("retry_after:30"), Some(30));
    }

    #[test]
    fn test_extract_retry_after_no_match() {
        assert_eq!(extract_retry_after("No retry info"), None);
        assert_eq!(extract_retry_after(""), None);
    }

    // ==================== DownloadStatus::get_emoji Tests ====================

    #[test]
    fn test_get_emoji_mp3() {
        assert_eq!(DownloadStatus::get_emoji(Some(&"mp3".to_string())), "🎵");
    }

    #[test]
    fn test_get_emoji_mp4() {
        assert_eq!(DownloadStatus::get_emoji(Some(&"mp4".to_string())), "🎬");
        assert_eq!(DownloadStatus::get_emoji(Some(&"mp4+mp3".to_string())), "🎬");
    }

    #[test]
    fn test_get_emoji_srt() {
        assert_eq!(DownloadStatus::get_emoji(Some(&"srt".to_string())), "📝");
    }

    #[test]
    fn test_get_emoji_txt() {
        assert_eq!(DownloadStatus::get_emoji(Some(&"txt".to_string())), "📄");
    }

    #[test]
    fn test_get_emoji_default() {
        assert_eq!(DownloadStatus::get_emoji(None), "🎵");
        assert_eq!(DownloadStatus::get_emoji(Some(&"unknown".to_string())), "🎵");
    }

    // ==================== DownloadStatus::to_message Tests ====================

    fn test_lang() -> LanguageIdentifier {
        crate::i18n::lang_from_code("ru")
    }

    #[test]
    fn test_status_starting_message() {
        let lang = test_lang();
        let status = DownloadStatus::Starting {
            title: "Test Song".to_string(),
            file_format: Some("mp3".to_string()),
        };
        let msg = status.to_message(&lang);
        assert!(msg.contains("Test Song"));
        assert!(msg.contains("⏳"));
    }

    #[test]
    fn test_status_downloading_message() {
        let lang = test_lang();
        let status = DownloadStatus::Downloading {
            title: "Test Song".to_string(),
            progress: 50,
            speed_mbs: Some(5.5),
            eta_seconds: Some(30),
            current_size: Some(50 * 1024 * 1024),
            total_size: Some(100 * 1024 * 1024),
            file_format: Some("mp3".to_string()),
        };
        let msg = status.to_message(&lang);
        assert!(msg.contains("Test Song"));
        assert!(msg.contains("50%"));
        assert!(msg.contains("📥"));
    }

    #[test]
    fn test_status_uploading_message() {
        let lang = test_lang();
        let status = DownloadStatus::Uploading {
            title: "Test Song".to_string(),
            dots: 2,
            progress: None,
            speed_mbs: None,
            eta_seconds: None,
            current_size: None,
            total_size: None,
            file_format: None,
        };
        let msg = status.to_message(&lang);
        assert!(msg.contains("Test Song"));
        assert!(msg.contains("📤"));
    }

    #[test]
    fn test_status_uploading_with_progress() {
        let lang = test_lang();
        let status = DownloadStatus::Uploading {
            title: "Test Song".to_string(),
            dots: 0,
            progress: Some(75),
            speed_mbs: Some(10.0),
            eta_seconds: Some(15),
            current_size: None,
            total_size: None,
            file_format: Some("mp4".to_string()),
        };
        let msg = status.to_message(&lang);
        assert!(msg.contains("75%"));
    }

    #[test]
    fn test_status_success_message() {
        let lang = test_lang();
        let status = DownloadStatus::Success {
            title: "Test Song".to_string(),
            elapsed_secs: 5,
            file_format: Some("mp3".to_string()),
        };
        let msg = status.to_message(&lang);
        assert!(msg.contains("Test Song"));
        assert!(msg.contains("✅"));
        assert!(msg.contains("5"));
    }

    #[test]
    fn test_status_completed_message() {
        let lang = test_lang();
        let status = DownloadStatus::Completed {
            title: "Test Song".to_string(),
            file_format: Some("mp3".to_string()),
        };
        let msg = status.to_message(&lang);
        assert!(msg.contains("Test Song"));
        assert!(msg.contains("🎵"));
    }

    #[test]
    fn test_status_error_message() {
        let lang = test_lang();
        let status = DownloadStatus::Error {
            title: "Test Song".to_string(),
            error: "Network error".to_string(),
            file_format: Some("mp3".to_string()),
        };
        let msg = status.to_message(&lang);
        assert!(msg.contains("Test Song"));
        assert!(msg.contains("❌"));
        assert!(msg.contains("Network error"));
    }

    #[test]
    fn test_status_message_english() {
        let lang = crate::i18n::lang_from_code("en");
        let status = DownloadStatus::Starting {
            title: "Test Song".to_string(),
            file_format: Some("mp3".to_string()),
        };
        let msg = status.to_message(&lang);
        assert!(msg.contains("Starting download"));
    }

    // ==================== ProgressMessage Tests ====================

    #[test]
    fn test_progress_message_new() {
        let lang = test_lang();
        let pm = ProgressMessage::new(ChatId(12345), lang);
        assert_eq!(pm.chat_id, ChatId(12345));
        assert!(pm.message_id.is_none());
    }
}
