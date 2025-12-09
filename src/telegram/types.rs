/// Информация о доступном формате видео
#[derive(Debug, Clone)]
pub struct VideoFormatInfo {
    pub quality: String,            // "1080p", "720p", "480p", "360p", "best"
    pub size_bytes: Option<u64>,    // размер в байтах
    pub resolution: Option<String>, // например "1920x1080"
}

/// Структура метаданных для превью
#[derive(Debug, Clone)]
pub struct PreviewMetadata {
    pub title: String,
    pub artist: String,
    pub thumbnail_url: Option<String>,
    pub duration: Option<u32>, // в секундах
    pub filesize: Option<u64>, // в байтах (для default формата)
    pub description: Option<String>,
    pub video_formats: Option<Vec<VideoFormatInfo>>, // доступные форматы видео (только для mp4)
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
