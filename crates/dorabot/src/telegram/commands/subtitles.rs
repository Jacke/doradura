use crate::core::config;
use crate::storage::db;

/// Result of a subtitle operation.
pub enum BurnSubsResult {
    /// Subtitles were burned successfully; contains the new video path.
    Burned(std::path::PathBuf),
    /// SRT file downloaded and ready for post-crop burning; contains the SRT path.
    SubtitleReady(std::path::PathBuf),
    /// No subtitles available for the requested language.
    NotFound,
    /// Subtitles exist but download/burn failed (rate-limit, network, ffmpeg error, etc.).
    Failed(String),
}

/// Download SRT subtitles via yt-dlp without burning them into the video.
/// Returns the SRT file path for post-crop burning in the circle filter chain.
pub async fn download_circle_subtitles(
    url: &str,
    lang: &str,
    temp_dir: &std::path::Path,
    chat_id: i64,
    source_id: i64,
) -> BurnSubsResult {
    use tokio::process::Command as TokioCommand;

    let srt_base = temp_dir.join(format!("subs_{}_{}", chat_id, source_id));
    let srt_base_str = srt_base.to_string_lossy().to_string();

    let ytdl_bin = &*config::YTDL_BIN;
    let mut cmd = TokioCommand::new(ytdl_bin);
    let mut args: Vec<&str> = vec![
        "--write-subs",
        "--write-auto-subs",
        "--sub-lang",
        lang,
        "--sub-format",
        "srt",
        "--convert-subs",
        "srt",
        "--skip-download",
        "--output",
        &srt_base_str,
        "--no-playlist",
    ];
    crate::download::metadata::add_cookies_args(&mut args);
    args.push(url);
    cmd.args(&args);

    let sub_result = crate::core::process::run_with_timeout(&mut cmd, config::download::ytdlp_timeout()).await;

    match sub_result {
        Ok(output) if output.status.success() => {
            let srt_dir = temp_dir.to_string_lossy().to_string();
            let srt_stem = format!("subs_{}_{}", chat_id, source_id);
            let srt_file = {
                let mut found = None;
                if let Ok(mut dir) = fs_err::tokio::read_dir(&srt_dir).await {
                    while let Ok(Some(entry)) = dir.next_entry().await {
                        let name = entry.file_name();
                        let name_str = name.to_string_lossy().into_owned();
                        if name_str.contains(&srt_stem) && name_str.ends_with(".srt") {
                            found = Some(entry.path());
                            break;
                        }
                    }
                }
                found
            };

            if let Some(sub_path) = srt_file {
                log::info!("Downloaded subtitles for circle: {:?}", sub_path);
                // Clean overlapping timestamps from YouTube auto-captions
                crate::download::downloader::clean_srt_overlaps(sub_path.to_str().unwrap_or_default()).await;
                BurnSubsResult::SubtitleReady(sub_path)
            } else {
                log::warn!("Subtitle file not found after yt-dlp download for circle");
                BurnSubsResult::NotFound
            }
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            log::warn!("yt-dlp subtitle download failed for circle: {}", stderr);
            if stderr.contains("has no subtitles") || stderr.contains("no subtitles") {
                return BurnSubsResult::NotFound;
            }
            BurnSubsResult::Failed(stderr)
        }
        Err(e) => {
            log::warn!("Failed to execute yt-dlp for circle subtitles: {}", e);
            BurnSubsResult::Failed(format!("{e}"))
        }
    }
}

/// Legacy: Download subtitles and burn them at original resolution.
/// Used by downloads.rs for non-circle subtitle burning.
pub async fn burn_circle_subtitles(
    url: &str,
    lang: &str,
    input_path: &std::path::Path,
    temp_dir: &std::path::Path,
    chat_id: i64,
    source_id: i64,
) -> BurnSubsResult {
    match download_circle_subtitles(url, lang, temp_dir, chat_id, source_id).await {
        BurnSubsResult::SubtitleReady(sub_path) => {
            let output_with_subs = temp_dir.join(format!("input_subs_{}_{}.mp4", chat_id, source_id));
            let circle_style = db::SubtitleStyle::circle_default();
            match crate::download::downloader::burn_subtitles_into_video(
                input_path.to_str().unwrap_or_default(),
                sub_path.to_str().unwrap_or_default(),
                output_with_subs.to_str().unwrap_or_default(),
                &circle_style,
            )
            .await
            {
                Ok(()) => {
                    log::info!("Burned subtitles into video source");
                    fs_err::tokio::remove_file(input_path).await.ok();
                    fs_err::tokio::remove_file(&sub_path).await.ok();
                    BurnSubsResult::Burned(output_with_subs)
                }
                Err(e) => {
                    log::warn!("Failed to burn subs: {}. Continuing without.", e);
                    fs_err::tokio::remove_file(&sub_path).await.ok();
                    fs_err::tokio::remove_file(&output_with_subs).await.ok();
                    BurnSubsResult::Failed(format!("ffmpeg error: {e}"))
                }
            }
        }
        other => other,
    }
}
