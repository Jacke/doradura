/// Available video format information
#[derive(Debug, Clone)]
pub struct VideoFormatInfo {
    pub quality: String,            // "1080p", "720p", "480p", "360p", "best"
    pub size_bytes: Option<u64>,    // size in bytes
    pub resolution: Option<String>, // e.g. "1920x1080"
}

/// Audio track language information extracted from yt-dlp metadata.
#[derive(Debug, Clone)]
pub struct AudioTrackInfo {
    pub language: String,
    pub display_name: Option<String>,
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
    pub audio_tracks: Option<Vec<AudioTrackInfo>>,   // available audio tracks (multi-language)
}

/// Extended metadata captured from `yt-dlp --dump-json` for the Info feature
/// (geo-availability card, full-metadata card, max-resolution thumbnail).
///
/// Populated alongside `PreviewMetadata` and stashed in `PREVIEW_CACHE` so the
/// info actions can read it without re-invoking yt-dlp.
#[derive(Debug, Clone, Default)]
pub struct ExtendedMetadata {
    /// Widest thumbnail URL from `thumbnails[]` (max-resolution variant —
    /// up to 1920x1080 for YouTube `maxresdefault.jpg`).
    pub thumbnail_max_url: Option<String>,
    /// `YYYYMMDD` from yt-dlp `upload_date` field.
    pub upload_date: Option<String>,
    pub view_count: Option<u64>,
    pub like_count: Option<u64>,
    pub comment_count: Option<u64>,
    /// Channel page URL (`channel_url` or `uploader_url`, fallback chain).
    pub channel_url: Option<String>,
    /// User-facing tags. Empty when source has none.
    pub tags: Vec<String>,
    /// User-facing categories. Empty when source has none.
    pub categories: Vec<String>,
    /// Full description (NOT truncated to 200 chars like `PreviewMetadata.description`).
    pub description_full: Option<String>,
    /// `availability` field — `"public"`, `"unlisted"`, `"premium_only"`,
    /// `"subscriber_only"`, `"needs_auth"`, `"public"`, etc. yt-dlp also uses
    /// `"public"` to mean "available", so callers should also check
    /// `geo_block` separately.
    pub availability: Option<String>,
    /// `true` when yt-dlp reports geo-blocking on this video.
    pub geo_block: bool,
    /// ISO-3166-1 alpha-2 codes the video is blocked in. Empty when no
    /// per-country block list is exposed by the source.
    pub blocked_countries: Vec<String>,
}

impl PreviewMetadata {
    /// Formats duration into human-readable format (MM:SS or H:MM:SS).
    pub fn format_duration(&self) -> String {
        match self.duration {
            Some(duration) => doracore::core::format_media_duration(duration as u64),
            None => "Unknown".to_string(),
        }
    }

    /// Formats file size into human-readable format (MB or KB)
    pub fn format_filesize(&self) -> String {
        match self.filesize {
            Some(size) => doracore::core::utils::format_bytes(size),
            None => "Unknown".to_string(),
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
            audio_tracks: None,
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
            audio_tracks: None,
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
            audio_tracks: None,
        };
        assert_eq!(meta.format_duration(), "Unknown");
    }

    #[test]
    fn test_preview_metadata_format_duration_hour_plus() {
        let meta = PreviewMetadata {
            title: "Test".to_string(),
            artist: "".to_string(),
            thumbnail_url: None,
            duration: Some(3661), // 1:01:01
            filesize: None,
            description: None,
            video_formats: None,
            timestamps: Vec::new(),
            is_photo: false,
            carousel_count: 0,
            audio_tracks: None,
        };
        assert_eq!(meta.format_duration(), "1:01:01");
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
            audio_tracks: None,
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
            audio_tracks: None,
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
            audio_tracks: None,
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
            audio_tracks: None,
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
            audio_tracks: None,
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
            audio_tracks: None,
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
            audio_tracks: None,
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
            audio_tracks: None,
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
            audio_tracks: None,
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
            audio_tracks: None,
        };

        assert!(meta.video_formats.is_some());
        assert_eq!(meta.video_formats.as_ref().unwrap().len(), 3);
    }
}
