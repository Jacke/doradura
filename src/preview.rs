use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, InputFile};
use url::Url;
use crate::error::AppError;
use crate::config;
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;
use base64::{Engine as _, engine::general_purpose::STANDARD};

/// Структура метаданных для превью
#[derive(Debug, Clone)]
pub struct PreviewMetadata {
    pub title: String,
    pub artist: String,
    pub thumbnail_url: Option<String>,
    pub duration: Option<u32>, // в секундах
    pub filesize: Option<u64>, // в байтах
    pub description: Option<String>,
}

impl PreviewMetadata {
    /// Форматирует длительность в читаемый формат (MM:SS)
    pub fn format_duration(&self) -> String {
        if let Some(duration) = self.duration {
            let minutes = duration / 60;
            let seconds = duration % 60;
            format!("{}:{:02}", minutes, seconds)
        } else {
            "Неизвестно".to_string()
        }
    }

    /// Форматирует размер файла в читаемый формат (MB или KB)
    pub fn format_filesize(&self) -> String {
        if let Some(size) = self.filesize {
            if size > 1024 * 1024 {
                format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
            } else if size > 1024 {
                format!("{:.1} KB", size as f64 / 1024.0)
            } else {
                format!("{} B", size)
            }
        } else {
            "Неизвестно".to_string()
        }
    }

    /// Возвращает отображаемое название (title или "artist - title")
    pub fn display_title(&self) -> String {
        if self.artist.trim().is_empty() {
            self.title.clone()
        } else {
            format!("{} - {}", self.artist, self.title)
        }
    }
}

/// Получает расширенные метаданные для превью
pub async fn get_preview_metadata(url: &Url) -> Result<PreviewMetadata, AppError> {
    let ytdl_bin = &*config::YTDL_BIN;
    log::debug!("Getting preview metadata for URL: {}", url);

    // Получаем title
    let title_output = timeout(
        config::download::ytdlp_timeout(),
        TokioCommand::new(ytdl_bin)
            .args(["--get-title", "--no-playlist", url.as_str()])
            .output()
    )
    .await
    .map_err(|_| AppError::Download("yt-dlp command timed out".to_string()))?
    .map_err(|e| AppError::Download(format!("Failed to get title: {}", e)))?;

    let title = if title_output.status.success() {
        String::from_utf8_lossy(&title_output.stdout).trim().to_string()
    } else {
        "Unknown Track".to_string()
    };

    // Получаем artist
    let artist_output = timeout(
        config::download::ytdlp_timeout(),
        TokioCommand::new(ytdl_bin)
            .args(["--print", "%(artist)s", "--no-playlist", url.as_str()])
            .output()
    )
    .await
    .map_err(|_| AppError::Download("yt-dlp command timed out".to_string()))?
    .map_err(|e| AppError::Download(format!("Failed to get artist: {}", e)))?;

    let artist = if artist_output.status.success() {
        String::from_utf8_lossy(&artist_output.stdout).trim().to_string()
    } else {
        String::new()
    };

    // Получаем thumbnail URL
    let thumbnail_output = timeout(
        config::download::ytdlp_timeout(),
        TokioCommand::new(ytdl_bin)
            .args(["--get-thumbnail", "--no-playlist", url.as_str()])
            .output()
    )
    .await
    .map_err(|_| AppError::Download("yt-dlp command timed out".to_string()))?
    .map_err(|e| AppError::Download(format!("Failed to get thumbnail: {}", e)))?;

    let thumbnail_url = if thumbnail_output.status.success() {
        let url_str = String::from_utf8_lossy(&thumbnail_output.stdout).trim().to_string();
        if url_str.is_empty() {
            None
        } else {
            Some(url_str)
        }
    } else {
        None
    };

    // Получаем duration
    let duration_output = timeout(
        config::download::ytdlp_timeout(),
        TokioCommand::new(ytdl_bin)
            .args(["--print", "%(duration)s", "--no-playlist", url.as_str()])
            .output()
    )
    .await
    .map_err(|_| AppError::Download("yt-dlp command timed out".to_string()))?
    .map_err(|e| AppError::Download(format!("Failed to get duration: {}", e)))?;

    let duration = if duration_output.status.success() {
        let duration_str = String::from_utf8_lossy(&duration_output.stdout);
        let duration_str = duration_str.trim();
        duration_str.parse::<f32>().ok().map(|d| d as u32)
    } else {
        None
    };

    // Получаем примерный размер файла (для аудио)
    let filesize_output = timeout(
        config::download::ytdlp_timeout(),
        TokioCommand::new(ytdl_bin)
            .args(["--print", "%(filesize)s", "--no-playlist", url.as_str()])
            .output()
    )
    .await
    .map_err(|_| AppError::Download("yt-dlp command timed out".to_string()))?
    .map_err(|e| AppError::Download(format!("Failed to get filesize: {}", e)))?;

    let filesize = if filesize_output.status.success() {
        let size_str = String::from_utf8_lossy(&filesize_output.stdout);
        let size_str = size_str.trim();
        size_str.parse::<u64>().ok()
    } else {
        None
    };

    // Получаем description (опционально)
    let description_output = timeout(
        config::download::ytdlp_timeout(),
        TokioCommand::new(ytdl_bin)
            .args(["--print", "%(description)s", "--no-playlist", url.as_str()])
            .output()
    )
    .await
    .ok(); // Не критично, игнорируем ошибки

    let description = description_output
        .and_then(|result| result.ok())
        .and_then(|out| {
            if out.status.success() {
                let desc = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if desc.is_empty() || desc == "NA" {
                    None
                } else {
                    // Ограничиваем длину описания
                    if desc.len() > 200 {
                        Some(format!("{}...", &desc[..200]))
                    } else {
                        Some(desc)
                    }
                }
            } else {
                None
            }
        });

    Ok(PreviewMetadata {
        title,
        artist,
        thumbnail_url,
        duration,
        filesize,
        description,
    })
}

/// Экранирует специальные символы для MarkdownV2
fn escape_markdown(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('_', "\\_")
        .replace('*', "\\*")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('(', "\\(")
        .replace(')', "\\)")
        .replace('~', "\\~")
        .replace('`', "\\`")
        .replace('>', "\\>")
        .replace('#', "\\#")
        .replace('+', "\\+")
        .replace('-', "\\-")
        .replace('=', "\\=")
        .replace('|', "\\|")
        .replace('{', "\\{")
        .replace('}', "\\}")
        .replace('.', "\\.")
        .replace('!', "\\!")
}

/// Отправляет превью с метаданными и кнопками подтверждения
pub async fn send_preview(
    bot: &Bot,
    chat_id: ChatId,
    url: &Url,
    metadata: &PreviewMetadata,
    default_format: &str,
) -> ResponseResult<Message> {
    // Формируем текст превью с экранированием
    let escaped_title = escape_markdown(&metadata.display_title());
    let mut text = format!("🎵 *{}*\n\n", escaped_title);
    
    if metadata.duration.is_some() {
        let duration_str = metadata.format_duration();
        text.push_str(&format!("⏱️ Длительность: {}\n", escape_markdown(&duration_str)));
    }
    
    if metadata.filesize.is_some() {
        let size_str = metadata.format_filesize();
        text.push_str(&format!("📦 Примерный размер: {}\n", escape_markdown(&size_str)));
    }
    
    if let Some(desc) = &metadata.description {
        text.push_str(&format!("\n📝 {}\n", escape_markdown(desc)));
    }
    
    text.push_str("\nВыбери действие\\:");
    
    // Создаем inline клавиатуру
    // Кодируем URL в base64 для передачи через callback
    let url_encoded = STANDARD.encode(url.as_str());
    
    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![InlineKeyboardButton::callback(
            format!("📥 Скачать ({})", match default_format {
                "mp3" => "MP3",
                "mp4" => "MP4",
                "srt" => "SRT",
                "txt" => "TXT",
                _ => "MP3",
            }),
            format!("download:{}:{}", default_format, url_encoded)
        )],
        vec![InlineKeyboardButton::callback(
            "⚙️ Настройки".to_string(),
            format!("preview:settings:{}", url_encoded)
        )],
        vec![InlineKeyboardButton::callback(
            "❌ Отмена".to_string(),
            format!("preview:cancel:{}", url_encoded)
        )],
    ]);

    // Отправляем превью с thumbnail если доступен
    if let Some(thumb_url) = &metadata.thumbnail_url {
        // Пытаемся отправить фото с thumbnail
        match reqwest::get(thumb_url).await {
            Ok(response) => {
                if response.status().is_success() {
                    match response.bytes().await {
                        Ok(bytes) => {
                            // Отправляем фото с описанием
                            let bytes_vec = bytes.to_vec();
                            return bot.send_photo(chat_id, InputFile::memory(bytes_vec))
                                .caption(text)
                                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                                .reply_markup(keyboard)
                                .await;
                        }
                        Err(e) => {
                            log::warn!("Failed to get thumbnail bytes: {}", e);
                        }
                    }
                }
            }
            Err(e) => {
                log::warn!("Failed to download thumbnail: {}", e);
            }
        }
    }

    // Если thumbnail не доступен, отправляем текстовое сообщение
    bot.send_message(chat_id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await
}

