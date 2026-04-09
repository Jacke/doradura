//! Video cover generation for MP3 downloads.
//!
//! Sends a separate visual message alongside an MP3: photo thumbnail,
//! GIF animation, or short MP4 clip from the original video source.

use super::CallbackCtx;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, InputFile, ParseMode};

/// Duration (seconds) of the GIF fragment extracted from the video.
const GIF_DURATION_SECS: u64 = 10;
/// Duration (seconds) of the MP4 video clip fragment.
const CLIP_DURATION_SECS: u64 = 15;
/// Width (pixels) for the generated GIF. Height preserves aspect ratio.
const GIF_WIDTH: u32 = 480;
/// Frames-per-second for the generated GIF.
const GIF_FPS: u8 = 12;
/// Maximum time (seconds) yt-dlp may take to download a fragment.
const YTDLP_FRAGMENT_TIMEOUT_SECS: u64 = 90;

/// Cover variant selected by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoverType {
    Photo,
    Gif,
    Clip,
}

impl CoverType {
    fn from_str_lossy(s: &str) -> Option<Self> {
        match s {
            "photo" => Some(Self::Photo),
            "gif" => Some(Self::Gif),
            "clip" => Some(Self::Clip),
            _ => None,
        }
    }
}

/// Show cover type picker: Photo / GIF / Video clip
pub(super) async fn handle(ctx: &CallbackCtx, action: &str, parts: &[&str]) -> ResponseResult<()> {
    match action {
        // downloads:cover:{download_id} → show picker
        "cover" => {
            if parts.len() < 3 {
                return Ok(());
            }
            let download_id = parts[2].parse::<i64>().unwrap_or(0);
            let kb = InlineKeyboardMarkup::new(vec![
                vec![
                    crate::telegram::cb(
                        "📸 Photo".to_string(),
                        format!("downloads:cover_do:photo:{}", download_id),
                    ),
                    crate::telegram::cb("🎞 GIF".to_string(), format!("downloads:cover_do:gif:{}", download_id)),
                    crate::telegram::cb(
                        "🎬 Video clip".to_string(),
                        format!("downloads:cover_do:clip:{}", download_id),
                    ),
                ],
                vec![crate::telegram::cb(
                    "❌ Cancel".to_string(),
                    "downloads:cancel".to_string(),
                )],
            ]);
            ctx.bot
                .edit_message_text(ctx.chat_id, ctx.message_id, "🖼 Choose cover type:")
                .reply_markup(kb)
                .await?;
        }
        // downloads:cover_do:{type}:{download_id}
        "cover_do" => {
            if parts.len() < 4 {
                return Ok(());
            }
            let Some(cover_type) = CoverType::from_str_lossy(parts[2]) else {
                return Ok(());
            };
            let download_id = parts[3].parse::<i64>().unwrap_or(0);

            let download = match ctx
                .shared_storage
                .get_download_history_entry(ctx.chat_id.0, download_id)
                .await
            {
                Ok(Some(d)) => d,
                _ => return Ok(()),
            };

            // Delete the picker message
            ctx.bot.delete_message(ctx.chat_id, ctx.message_id).await.ok();

            let url = download.url.clone();
            let title = download.title.clone();
            let bot = ctx.bot.clone();
            let chat_id = ctx.chat_id;

            tokio::spawn(async move {
                if let Err(e) = generate_and_send_cover(&bot, chat_id, &url, &title, cover_type).await {
                    log::error!("Cover generation failed: {}", e);
                    bot.send_message(chat_id, format!("❌ Cover generation failed: {}", e))
                        .await
                        .ok();
                }
            });
        }
        _ => {}
    }
    Ok(())
}

async fn generate_and_send_cover(
    bot: &crate::telegram::Bot,
    chat_id: ChatId,
    url: &str,
    title: &str,
    cover_type: CoverType,
) -> Result<(), anyhow::Error> {
    use crate::core::utils::TempDirGuard;

    let status = bot.send_message(chat_id, "⏳ Generating cover...").await?;

    let mut guard = TempDirGuard::new("doradura_cover").await?;

    match cover_type {
        CoverType::Photo => {
            // Download YouTube thumbnail
            let thumb_url = resolve_thumbnail_url(url);
            if let Some(thumb_url) = thumb_url {
                let resp = reqwest::get(&thumb_url).await?;
                if resp.status().is_success() {
                    let bytes = resp.bytes().await?;
                    let photo_path = guard.path().join("cover.jpg");

                    // Convert to JPEG if needed
                    let format = doracore::download::thumbnail::detect_image_format(&bytes);
                    let final_bytes = if matches!(format, doracore::download::thumbnail::ImageFormat::WebP) {
                        match doracore::download::thumbnail::convert_webp_to_jpeg(&bytes).await {
                            Ok(jpeg_bytes) => jpeg_bytes,
                            Err(_) => bytes.to_vec(),
                        }
                    } else {
                        bytes.to_vec()
                    };

                    tokio::fs::write(&photo_path, &final_bytes).await?;
                    bot.delete_message(chat_id, status.id).await.ok();
                    bot.send_photo(chat_id, InputFile::file(&photo_path))
                        .caption(cover_caption("🖼", title, url))
                        .parse_mode(ParseMode::Html)
                        .await?;
                    return Ok(());
                }
            }
            bot.delete_message(chat_id, status.id).await.ok();
            bot.send_message(chat_id, "❌ Could not fetch video thumbnail.").await?;
        }
        CoverType::Gif => {
            let video_path = download_video_fragment(url, GIF_DURATION_SECS, guard.path()).await?;
            guard.track_file(video_path.clone());

            bot.edit_message_text(chat_id, status.id, "🎞 Converting to GIF...")
                .await
                .ok();

            let gif_path = doracore::conversion::video::to_gif(
                &video_path,
                doracore::conversion::video::GifOptions {
                    duration: Some(GIF_DURATION_SECS),
                    start_time: None,
                    width: Some(GIF_WIDTH),
                    fps: Some(GIF_FPS),
                },
            )
            .await?;
            guard.track_file(gif_path.clone());

            bot.delete_message(chat_id, status.id).await.ok();
            bot.send_animation(chat_id, InputFile::file(&gif_path))
                .caption(cover_caption("🎞", title, url))
                .parse_mode(ParseMode::Html)
                .await?;
        }
        CoverType::Clip => {
            let video_path = download_video_fragment(url, CLIP_DURATION_SECS, guard.path()).await?;
            guard.track_file(video_path.clone());

            bot.delete_message(chat_id, status.id).await.ok();
            bot.send_video(chat_id, InputFile::file(&video_path))
                .caption(cover_caption("🎬", title, url))
                .parse_mode(ParseMode::Html)
                .await?;
        }
    }

    Ok(())
}

/// Escape a string for safe interpolation into HTML text content.
fn html_escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&#39;")
}

/// Escape a URL for safe interpolation into an HTML attribute value.
fn html_escape_attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('\'', "&#39;")
}

/// Build caption with a small link to the source video.
/// Uses HTML `<a href="url">🔗</a>` so the link is visible but compact,
/// and no link preview is shown since the media message itself is the preview.
fn cover_caption(emoji: &str, title: &str, url: &str) -> String {
    format!(
        "{} {} <a href=\"{}\">🔗</a>",
        emoji,
        html_escape_text(title),
        html_escape_attr(url)
    )
}

/// Resolve thumbnail URL from video URL (YouTube → maxresdefault.jpg).
/// Returns None for non-YouTube URLs.
fn resolve_thumbnail_url(url: &str) -> Option<String> {
    doracore::download::fast_metadata::extract_youtube_id(url)
        .map(|id| format!("https://img.youtube.com/vi/{}/maxresdefault.jpg", id))
}

/// Download first N seconds of video using yt-dlp with full proxy/cookie support.
///
/// Uses the same arg infrastructure as the main download pipeline so it works
/// on Railway (where YouTube IPs are blocked without proxy).
async fn download_video_fragment(
    url: &str,
    duration_secs: u64,
    work_dir: &std::path::Path,
) -> Result<std::path::PathBuf, anyhow::Error> {
    use doracore::download::metadata;
    use tokio::process::Command;

    let output_path = work_dir.join("fragment.mp4");
    let output_str = output_path.to_string_lossy().to_string();

    // Collect cookie/proxy args first (borrows static strings)
    let mut cookie_args: Vec<&str> = Vec::new();
    metadata::add_cookies_args(&mut cookie_args);
    let cookie_args_owned: Vec<String> = cookie_args.iter().map(|s| s.to_string()).collect();

    // Build yt-dlp args with cookies + proxy (same as main pipeline)
    let mut args: Vec<String> = vec![
        "--no-playlist".into(),
        "-f".into(),
        "best[height<=720]/best".into(),
        "--download-sections".into(),
        format!("*0-{}", duration_secs),
        "--force-keyframes-at-cuts".into(),
        "-o".into(),
        output_str,
        "--no-part".into(),
        "--retries".into(),
        "3".into(),
        "--socket-timeout".into(),
        "30".into(),
        "--no-check-certificate".into(),
        "--no-warnings".into(),
    ];

    // Add extractor args for YouTube
    let extractor_args = metadata::default_youtube_extractor_args();
    args.push("--extractor-args".into());
    args.push(extractor_args.into());

    // Add cookies + proxy from the main pipeline infrastructure
    args.extend(cookie_args_owned);

    args.push(url.into());

    log::info!(
        "[cover] Downloading {}s fragment from {} ({} args)",
        duration_secs,
        &url[..url.len().min(60)],
        args.len()
    );

    let yt_result = tokio::time::timeout(
        std::time::Duration::from_secs(YTDLP_FRAGMENT_TIMEOUT_SECS),
        Command::new("yt-dlp").args(&args).output(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("yt-dlp timed out after {}s", YTDLP_FRAGMENT_TIMEOUT_SECS))??;

    if !yt_result.status.success() {
        let stderr = String::from_utf8_lossy(&yt_result.stderr);
        log::error!("[cover] yt-dlp fragment download failed: {}", stderr);
        return Err(anyhow::anyhow!("yt-dlp failed: {}", &stderr[..stderr.len().min(200)]));
    }

    if !output_path.exists() {
        return Err(anyhow::anyhow!("yt-dlp produced no output file"));
    }

    let size = tokio::fs::metadata(&output_path).await?.len();
    log::info!("[cover] Fragment downloaded: {} bytes", size);

    Ok(output_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── cover_caption ──────────────────────────────────────────────

    #[test]
    fn cover_caption_basic() {
        let cap = cover_caption("🖼", "Song Title", "https://youtu.be/abc");
        assert!(cap.contains("🖼 Song Title"));
        assert!(cap.contains("<a href=\"https://youtu.be/abc\">🔗</a>"));
    }

    #[test]
    fn cover_caption_escapes_html_in_title() {
        let cap = cover_caption("📸", "Tom & Jerry <3>", "https://youtu.be/x");
        assert!(cap.contains("Tom &amp; Jerry &lt;3&gt;"));
        assert!(!cap.contains("<3>"));
    }

    #[test]
    fn cover_caption_escapes_ampersand_in_url() {
        let cap = cover_caption("🎬", "Title", "https://youtube.com/watch?v=a&t=10");
        assert!(cap.contains("href=\"https://youtube.com/watch?v=a&amp;t=10\""));
    }

    #[test]
    fn cover_caption_escapes_quotes_in_url() {
        let cap = cover_caption("🎞", "T", "https://example.com/\"test\"");
        assert!(cap.contains("href=\"https://example.com/&quot;test&quot;\""));
    }

    #[test]
    fn cover_caption_empty_title() {
        let cap = cover_caption("📸", "", "https://youtu.be/x");
        assert!(cap.contains("📸 "));
        assert!(cap.contains("🔗</a>"));
    }

    #[test]
    fn cover_caption_escapes_apostrophe_in_title() {
        let cap = cover_caption("🖼", "Don't Stop", "https://youtu.be/x");
        assert!(cap.contains("Don&#39;t Stop"));
        assert!(!cap.contains("Don't"));
    }

    // ── CoverType ─────────────────────────────────────────────────

    #[test]
    fn cover_type_from_str() {
        assert_eq!(CoverType::from_str_lossy("photo"), Some(CoverType::Photo));
        assert_eq!(CoverType::from_str_lossy("gif"), Some(CoverType::Gif));
        assert_eq!(CoverType::from_str_lossy("clip"), Some(CoverType::Clip));
        assert_eq!(CoverType::from_str_lossy("invalid"), None);
        assert_eq!(CoverType::from_str_lossy(""), None);
    }

    // ── resolve_thumbnail_url ──────────────────────────────────────

    #[test]
    fn resolve_thumbnail_youtube_standard() {
        let url = resolve_thumbnail_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ");
        assert_eq!(url.unwrap(), "https://img.youtube.com/vi/dQw4w9WgXcQ/maxresdefault.jpg");
    }

    #[test]
    fn resolve_thumbnail_youtube_short_link() {
        let url = resolve_thumbnail_url("https://youtu.be/dQw4w9WgXcQ");
        assert_eq!(url.unwrap(), "https://img.youtube.com/vi/dQw4w9WgXcQ/maxresdefault.jpg");
    }

    #[test]
    fn resolve_thumbnail_youtube_shorts() {
        let url = resolve_thumbnail_url("https://www.youtube.com/shorts/abc123");
        assert_eq!(url.unwrap(), "https://img.youtube.com/vi/abc123/maxresdefault.jpg");
    }

    #[test]
    fn resolve_thumbnail_non_youtube_returns_none() {
        assert!(resolve_thumbnail_url("https://soundcloud.com/artist/track").is_none());
        assert!(resolve_thumbnail_url("https://vimeo.com/123456").is_none());
        assert!(resolve_thumbnail_url("https://instagram.com/p/abc").is_none());
    }

    #[test]
    fn resolve_thumbnail_empty_url() {
        assert!(resolve_thumbnail_url("").is_none());
    }

    #[test]
    fn resolve_thumbnail_invalid_url() {
        assert!(resolve_thumbnail_url("not a url at all").is_none());
    }

    // ── download_video_fragment (integration, requires yt-dlp + ffmpeg) ──

    #[tokio::test]
    #[ignore] // requires yt-dlp + ffmpeg + network
    async fn download_fragment_produces_file() {
        let dir = tempfile::tempdir().unwrap();
        let result = download_video_fragment("https://youtu.be/jNQXAC9IVRw", 5, dir.path()).await;
        assert!(result.is_ok(), "download_video_fragment failed: {:?}", result.err());
        let path = result.unwrap();
        assert!(path.exists());
        let meta = tokio::fs::metadata(&path).await.unwrap();
        assert!(meta.len() > 1000, "fragment file too small: {} bytes", meta.len());
    }

    #[tokio::test]
    #[ignore]
    async fn download_fragment_invalid_url_fails() {
        let dir = tempfile::tempdir().unwrap();
        let result = download_video_fragment("https://youtube.com/watch?v=NONEXISTENT999", 5, dir.path()).await;
        assert!(result.is_err());
    }
}
