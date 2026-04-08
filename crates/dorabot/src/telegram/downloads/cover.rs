//! Video cover generation for MP3 downloads.
//!
//! Sends a separate visual message alongside an MP3: photo thumbnail,
//! GIF animation, or short MP4 clip from the original video source.

use super::CallbackCtx;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, InputFile};

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
            let cover_type = parts[2]; // "photo", "gif", "clip"
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
            let shared_storage = ctx.shared_storage.clone();
            let cover_type = cover_type.to_string();

            tokio::spawn(async move {
                if let Err(e) = generate_and_send_cover(&bot, chat_id, &url, &title, &cover_type, &shared_storage).await
                {
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
    cover_type: &str,
    _shared_storage: &std::sync::Arc<crate::storage::SharedStorage>,
) -> Result<(), anyhow::Error> {
    use crate::core::utils::TempDirGuard;

    let status = bot.send_message(chat_id, "⏳ Generating cover...").await?;

    let mut guard = TempDirGuard::new("doradura_cover").await?;

    match cover_type {
        "photo" => {
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
                        .caption(format!("🖼 {}", title))
                        .await?;
                    return Ok(());
                }
            }
            bot.delete_message(chat_id, status.id).await.ok();
            bot.send_message(chat_id, "❌ Could not fetch video thumbnail.").await?;
        }
        "gif" => {
            // Download first 10s of video, convert to GIF
            let video_path = download_video_fragment(url, 10, guard.path()).await?;
            guard.track_file(video_path.clone());

            bot.edit_message_text(chat_id, status.id, "🎞 Converting to GIF...")
                .await
                .ok();

            let gif_path = doracore::conversion::video::to_gif(
                &video_path,
                doracore::conversion::video::GifOptions {
                    duration: Some(10),
                    start_time: None,
                    width: Some(480),
                    fps: Some(12),
                },
            )
            .await?;
            guard.track_file(gif_path.clone());

            bot.delete_message(chat_id, status.id).await.ok();
            bot.send_animation(chat_id, InputFile::file(&gif_path))
                .caption(format!("🎞 {}", title))
                .await?;
        }
        "clip" => {
            // Download first 15s of video as MP4
            let video_path = download_video_fragment(url, 15, guard.path()).await?;
            guard.track_file(video_path.clone());

            bot.delete_message(chat_id, status.id).await.ok();
            bot.send_video(chat_id, InputFile::file(&video_path))
                .caption(format!("🎬 {}", title))
                .await?;
        }
        _ => {
            bot.delete_message(chat_id, status.id).await.ok();
        }
    }

    Ok(())
}

/// Resolve thumbnail URL from video URL (YouTube → maxresdefault.jpg)
fn resolve_thumbnail_url(url: &str) -> Option<String> {
    doracore::download::fast_metadata::extract_youtube_id(url)
        .map(|id| format!("https://img.youtube.com/vi/{}/maxresdefault.jpg", id))
}

/// Download first N seconds of video using yt-dlp + ffmpeg
async fn download_video_fragment(
    url: &str,
    duration_secs: u64,
    work_dir: &std::path::Path,
) -> Result<std::path::PathBuf, anyhow::Error> {
    use tokio::process::Command;

    let output_path = work_dir.join("fragment.mp4");

    // Use yt-dlp to get the direct video URL, then ffmpeg to download a fragment
    let yt_output = Command::new("yt-dlp")
        .args(["--no-playlist", "-f", "best[height<=720]", "--get-url", url])
        .output()
        .await?;

    let direct_url = String::from_utf8_lossy(&yt_output.stdout).trim().to_string();
    if direct_url.is_empty() {
        return Err(anyhow::anyhow!("Could not resolve video URL"));
    }

    // Use first URL if multiple lines (video + audio)
    let first_url = direct_url.lines().next().unwrap_or(&direct_url);

    let ffmpeg_result = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-i",
                first_url,
                "-t",
                &duration_secs.to_string(),
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-crf",
                "28",
                "-c:a",
                "aac",
                "-b:a",
                "128k",
                "-movflags",
                "+faststart",
            ])
            .arg(&output_path)
            .output(),
    )
    .await??;

    if !ffmpeg_result.status.success() {
        let stderr = String::from_utf8_lossy(&ffmpeg_result.stderr);
        return Err(anyhow::anyhow!("ffmpeg failed: {}", stderr));
    }

    Ok(output_path)
}
