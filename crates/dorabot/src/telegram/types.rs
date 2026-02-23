/// Available video format information
#[derive(Debug, Clone)]
pub struct VideoFormatInfo {
    pub quality: String,            // "1080p", "720p", "480p", "360p", "best"
    pub size_bytes: Option<u64>,    // size in bytes
    pub resolution: Option<String>, // e.g. "1920x1080"
}

/// Preview metadata structure
#[derive(Debug, Clone)]
pub struct PreviewMetadata {
    pub title: String,
    pub artist: String,
    pub thumbnail_url: Option<String>,
    pub duration: Option<u32>, // in seconds
    pub filesize: Option<u64>, // in bytes (for default format)
    pub description: Option<String>,
    pub video_formats: Option<Vec<VideoFormatInfo>>, // available video formats (mp4 only)
    pub timestamps: Vec<crate::timestamps::VideoTimestamp>, // extracted timestamps
    pub is_photo: bool,                              // true for Instagram photo posts (no audio/video to extract)
    pub carousel_count: u8,                          // 0 = not carousel, 2-10 = carousel item count
}

impl PreviewMetadata {
    /// Formats duration into human-readable format (MM:SS)
    pub fn format_duration(&self) -> String {
        if let Some(duration) = self.duration {
            let minutes = duration / 60;
            let seconds = duration % 60;
            format!("{}:{:02}", minutes, seconds)
        } else {
            "Unknown".to_string()
        }
    }

    /// Formats file size into human-readable format (MB or KB)
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
            "Unknown".to_string()
        }
    }

    /// Returns display title (title or "artist - title")
    pub fn display_title(&self) -> String {
        if self.artist.trim().is_empty() {
            self.title.clone()
        } else {
            format!("{} - {}", self.artist, self.title)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_video_format_info_debug() {
        let info = VideoFormatInfo {
            quality: "1080p".to_string(),
            size_bytes: Some(1_000_000),
            resolution: Some("1920x1080".to_string()),
        };
        let debug = format!("{:?}", info);
        assert!(debug.contains("VideoFormatInfo"));
        assert!(debug.contains("1080p"));
    }

    #[test]
    fn test_video_format_info_clone() {
        let info = VideoFormatInfo {
            quality: "720p".to_string(),
            size_bytes: Some(500_000),
            resolution: Some("1280x720".to_string()),
        };
        let cloned = info.clone();
        assert_eq!(info.quality, cloned.quality);
        assert_eq!(info.size_bytes, cloned.size_bytes);
        assert_eq!(info.resolution, cloned.resolution);
    }

    #[test]
    fn test_preview_metadata_format_duration() {
        let meta = PreviewMetadata {
            title: "Test".to_string(),
            artist: "Artist".to_string(),
            thumbnail_url: None,
            duration: Some(185), // 3:05
            filesize: None,
            description: None,
            video_formats: None,
            timestamps: Vec::new(),
            is_photo: false,
            carousel_count: 0,
        };
        assert_eq!(meta.format_duration(), "3:05");
    }

    #[test]
    fn test_preview_metadata_format_duration_zero() {
        let meta = PreviewMetadata {
            title: "Test".to_string(),
            artist: "".to_string(),
            thumbnail_url: None,
            duration: Some(0),
            filesize: None,
            description: None,
            video_formats: None,
            timestamps: Vec::new(),
            is_photo: false,
            carousel_count: 0,
        };
        assert_eq!(meta.format_duration(), "0:00");
    }

    #[test]
    fn test_preview_metadata_format_duration_none() {
        let meta = PreviewMetadata {
            title: "Test".to_string(),
            artist: "".to_string(),
            thumbnail_url: None,
            duration: None,
            filesize: None,
            description: None,
            video_formats: None,
            timestamps: Vec::new(),
            is_photo: false,
            carousel_count: 0,
        };
        assert_eq!(meta.format_duration(), "Unknown");
    }

    #[test]
    fn test_preview_metadata_format_duration_hour_plus() {
        let meta = PreviewMetadata {
            title: "Test".to_string(),
            artist: "".to_string(),
            thumbnail_url: None,
            duration: Some(3661), // 61:01
            filesize: None,
            description: None,
            video_formats: None,
            timestamps: Vec::new(),
            is_photo: false,
            carousel_count: 0,
        };
        assert_eq!(meta.format_duration(), "61:01");
    }

    #[test]
    fn test_preview_metadata_format_filesize_mb() {
        let meta = PreviewMetadata {
            title: "Test".to_string(),
            artist: "".to_string(),
            thumbnail_url: None,
            duration: None,
            filesize: Some(5 * 1024 * 1024), // 5 MB
            description: None,
            video_formats: None,
            timestamps: Vec::new(),
            is_photo: false,
            carousel_count: 0,
        };
        assert_eq!(meta.format_filesize(), "5.0 MB");
    }

    #[test]
    fn test_preview_metadata_format_filesize_kb() {
        let meta = PreviewMetadata {
            title: "Test".to_string(),
            artist: "".to_string(),
            thumbnail_url: None,
            duration: None,
            filesize: Some(512 * 1024), // 512 KB
            description: None,
            video_formats: None,
            timestamps: Vec::new(),
            is_photo: false,
            carousel_count: 0,
        };
        assert_eq!(meta.format_filesize(), "512.0 KB");
    }

    #[test]
    fn test_preview_metadata_format_filesize_bytes() {
        let meta = PreviewMetadata {
            title: "Test".to_string(),
            artist: "".to_string(),
            thumbnail_url: None,
            duration: None,
            filesize: Some(500), // 500 bytes
            description: None,
            video_formats: None,
            timestamps: Vec::new(),
            is_photo: false,
            carousel_count: 0,
        };
        assert_eq!(meta.format_filesize(), "500 B");
    }

    #[test]
    fn test_preview_metadata_format_filesize_none() {
        let meta = PreviewMetadata {
            title: "Test".to_string(),
            artist: "".to_string(),
            thumbnail_url: None,
            duration: None,
            filesize: None,
            description: None,
            video_formats: None,
            timestamps: Vec::new(),
            is_photo: false,
            carousel_count: 0,
        };
        assert_eq!(meta.format_filesize(), "Unknown");
    }

    #[test]
    fn test_preview_metadata_display_title_with_artist() {
        let meta = PreviewMetadata {
            title: "Song Name".to_string(),
            artist: "Artist Name".to_string(),
            thumbnail_url: None,
            duration: None,
            filesize: None,
            description: None,
            video_formats: None,
            timestamps: Vec::new(),
            is_photo: false,
            carousel_count: 0,
        };
        assert_eq!(meta.display_title(), "Artist Name - Song Name");
    }

    #[test]
    fn test_preview_metadata_display_title_without_artist() {
        let meta = PreviewMetadata {
            title: "Song Name".to_string(),
            artist: "".to_string(),
            thumbnail_url: None,
            duration: None,
            filesize: None,
            description: None,
            video_formats: None,
            timestamps: Vec::new(),
            is_photo: false,
            carousel_count: 0,
        };
        assert_eq!(meta.display_title(), "Song Name");
    }

    #[test]
    fn test_preview_metadata_display_title_whitespace_artist() {
        let meta = PreviewMetadata {
            title: "Song Name".to_string(),
            artist: "   ".to_string(),
            thumbnail_url: None,
            duration: None,
            filesize: None,
            description: None,
            video_formats: None,
            timestamps: Vec::new(),
            is_photo: false,
            carousel_count: 0,
        };
        assert_eq!(meta.display_title(), "Song Name");
    }

    #[test]
    fn test_preview_metadata_debug() {
        let meta = PreviewMetadata {
            title: "Test".to_string(),
            artist: "Artist".to_string(),
            thumbnail_url: Some("https://example.com/thumb.jpg".to_string()),
            duration: Some(180),
            filesize: Some(1000000),
            description: Some("Description".to_string()),
            video_formats: None,
            timestamps: Vec::new(),
            is_photo: false,
            carousel_count: 0,
        };
        let debug = format!("{:?}", meta);
        assert!(debug.contains("PreviewMetadata"));
        assert!(debug.contains("Test"));
    }

    #[test]
    fn test_preview_metadata_clone() {
        let meta = PreviewMetadata {
            title: "Test".to_string(),
            artist: "Artist".to_string(),
            thumbnail_url: Some("url".to_string()),
            duration: Some(100),
            filesize: Some(1000),
            description: Some("desc".to_string()),
            video_formats: Some(vec![VideoFormatInfo {
                quality: "720p".to_string(),
                size_bytes: Some(500),
                resolution: Some("1280x720".to_string()),
            }]),
            timestamps: Vec::new(),
            is_photo: false,
            carousel_count: 0,
        };
        let cloned = meta.clone();
        assert_eq!(meta.title, cloned.title);
        assert_eq!(meta.artist, cloned.artist);
        assert_eq!(meta.duration, cloned.duration);
    }

    #[test]
    fn test_preview_metadata_with_video_formats() {
        let meta = PreviewMetadata {
            title: "Video".to_string(),
            artist: "".to_string(),
            thumbnail_url: None,
            duration: Some(600),
            filesize: None,
            description: None,
            video_formats: Some(vec![
                VideoFormatInfo {
                    quality: "1080p".to_string(),
                    size_bytes: Some(100_000_000),
                    resolution: Some("1920x1080".to_string()),
                },
                VideoFormatInfo {
                    quality: "720p".to_string(),
                    size_bytes: Some(50_000_000),
                    resolution: Some("1280x720".to_string()),
                },
                VideoFormatInfo {
                    quality: "480p".to_string(),
                    size_bytes: Some(25_000_000),
                    resolution: Some("854x480".to_string()),
                },
            ]),
            timestamps: Vec::new(),
            is_photo: false,
            carousel_count: 0,
        };

        assert!(meta.video_formats.is_some());
        assert_eq!(meta.video_formats.as_ref().unwrap().len(), 3);
    }
}
