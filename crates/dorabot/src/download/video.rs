//! Video download and processing module
//!
//! Uses the unified download pipeline for the download phase,
//! with video-specific post-processing: stream verification,
//! subtitle burning, video splitting, and multi-part sending.

use crate::core::config;
use crate::core::error::AppError;
use crate::core::metrics;
use crate::core::process::{run_with_timeout, FFPROBE_TIMEOUT};
use crate::core::rate_limiter::RateLimiter;
use crate::core::types::Plan;
use crate::download::downloader::{burn_subtitles_into_video, split_video_into_parts};
use crate::download::error::DownloadError;
use crate::download::metadata::{
    add_cookies_args, find_actual_downloaded_file, has_both_video_and_audio, probe_video_metadata,
};
use crate::download::pipeline::{self, DownloadPhaseResult, PipelineFormat};
use crate::download::progress::{DownloadStatus, ProgressBarStyle, ProgressMessage};
use crate::download::send::{send_error_with_sticker, send_video_with_retry};
use crate::download::source::bot_global;
use crate::storage::db::DbPool;
use crate::storage::SharedStorage;
use crate::telegram::cache::PREVIEW_CACHE;
use crate::telegram::Bot;
use chrono::{DateTime, Utc};
use std::fs;
use std::sync::Arc;
use teloxide::prelude::*;
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;
use tracing::Instrument;
use url::Url;

/// Download video file and send it to user
///
/// Downloads video from URL using the unified download pipeline, performs video-specific
/// post-processing (stream verification, subtitle burning, splitting), and sends
/// the file to the user via Telegram.
pub async fn download_and_send_video(
    bot: Bot,
    chat_id: ChatId,
    url: Url,
    rate_limiter: Arc<RateLimiter>,
    _created_timestamp: DateTime<Utc>,
    db_pool: Option<Arc<DbPool>>,
    shared_storage: Option<Arc<SharedStorage>>,
    video_quality: Option<String>,
    message_id: Option<i32>,
    alert_manager: Option<Arc<crate::core::alerts::AlertManager>>,
    time_range: Option<(String, String)>,
) -> ResponseResult<()> {
    let bot_clone = bot.clone();
    let _rate_limiter = rate_limiter;
    let _db_pool = db_pool; // kept for API compatibility; subtitles now use SharedStorage
    let shared_storage_clone = shared_storage.clone();

    // Inherit the parent span (from queue_processor) so all video logs carry op=...
    let span = tracing::Span::current();
    tokio::spawn(
        async move {
            let lang = if let Some(ref storage) = shared_storage_clone {
                crate::i18n::user_lang_from_storage(storage, chat_id.0).await
            } else {
                crate::i18n::lang_from_code("ru")
            };
            let mut progress_msg = ProgressMessage::new(chat_id, lang.clone());
            if let Some(ref storage) = shared_storage_clone {
                if let Ok(style_str) = storage.get_user_progress_bar_style(chat_id.0).await {
                    progress_msg.style = ProgressBarStyle::parse(&style_str);
                }
            }
            let start_time = std::time::Instant::now();

            // Metrics setup
            let user_plan = if let Some(ref storage) = shared_storage_clone {
                storage
                    .get_user(chat_id.0)
                    .await
                    .ok()
                    .flatten()
                    .map(|u| u.plan)
                    .unwrap_or_default()
            } else {
                Plan::default()
            };
            metrics::record_format_request("mp4", user_plan.as_str());
            metrics::record_platform_download(metrics::extract_platform(url.as_str()));

            let quality = video_quality.as_deref().unwrap_or("default");
            let timer = metrics::DOWNLOAD_DURATION_SECONDS
                .with_label_values(&["mp4", quality])
                .start_timer();

            // Read audio_lang from preview context (set by the audio track picker button)
            let audio_lang = if let Some(ref storage) = shared_storage_clone {
                storage
                    .get_preview_context(chat_id.0, url.as_str())
                    .await
                    .ok()
                    .flatten()
                    .and_then(|ctx| ctx.audio_lang)
            } else {
                None
            };

            if let Some(ref lang) = audio_lang {
                log::info!("🔊 Audio track language selected: '{}' for {}", lang, url);
            }

            let format = PipelineFormat::Video {
                quality: video_quality.clone(),
                time_range,
            };
            let registry = bot_global();

            // Global timeout for entire operation
            let result: Result<(), AppError> = match timeout(config::download::global_timeout(), async {
                // ── Phase 1: Download via pipeline ──
                let phase = pipeline::download_phase(
                    &bot_clone,
                    chat_id,
                    &url,
                    &format,
                    registry,
                    &mut progress_msg,
                    message_id,
                    shared_storage_clone.as_ref(),
                )
                .await
                .map_err(|e| e.into_app_error())?;

                let DownloadPhaseResult {
                    output: download_output,
                    title,
                    artist,
                    display_title,
                    caption,
                } = phase;

                // ── Phase 2: Video-specific post-processing ──

                // Get thumbnail URL (best-effort, non-blocking)
                let thumbnail_url = get_thumbnail_url(&url).await;

                // Find actual downloaded file (yt-dlp may rename with suffixes)
                let actual_file_path = find_actual_downloaded_file(&download_output.file_path)
                    .unwrap_or_else(|_| download_output.file_path.clone());

                // Verify both video and audio streams are present
                match has_both_video_and_audio(&actual_file_path) {
                    Ok(true) => {
                        log::info!("Video verified: both streams present");
                    }
                    Ok(false) => {
                        log::error!("Video file missing video or audio stream!");
                        let mut probe_cmd = TokioCommand::new("ffprobe");
                        probe_cmd.args(["-v", "error", "-show_streams", &*actual_file_path]);
                        if let Ok(output) = run_with_timeout(&mut probe_cmd, FFPROBE_TIMEOUT).await {
                            log::error!("Streams info: {}", String::from_utf8_lossy(&output.stdout));
                        }
                        send_error_with_sticker(&bot_clone, chat_id).await;
                        let _ = progress_msg
                            .update(
                                &bot_clone,
                                DownloadStatus::Error {
                                    title: display_title.as_ref().to_string(),
                                    error: "Video file is corrupted or missing required tracks".to_string(),
                                    file_format: Some("mp4".to_string()),
                                },
                            )
                            .await;
                        return Err(AppError::Download(DownloadError::Other(
                            "Video file missing video or audio stream".to_string(),
                        )));
                    }
                    Err(e) => {
                        log::warn!("Failed to verify video streams: {}. Continuing...", e);
                    }
                }

                // Replace audio track if user selected a specific language
                let actual_file_path = if let Some(ref lang) = audio_lang {
                    let audio_replace_start = std::time::Instant::now();
                    let result = replace_audio_track(&actual_file_path, &url, lang).await;
                    log::info!(
                        "⏱️ [AUDIO_REPLACE] done in {:.1}s",
                        audio_replace_start.elapsed().as_secs_f64()
                    );
                    result
                } else {
                    actual_file_path
                };

                // Burn subtitles if user has the setting enabled
                let subs_start = std::time::Instant::now();
                let actual_file_path =
                    maybe_burn_subtitles(&actual_file_path, &url, shared_storage_clone.as_ref(), chat_id).await;
                if subs_start.elapsed().as_secs_f64() > 1.0 {
                    log::info!("⏱️ [BURN_SUBS] done in {:.1}s", subs_start.elapsed().as_secs_f64());
                }

                // Get user preference for send_as_document
                let send_as_document = if let Some(ref storage) = shared_storage_clone {
                    storage
                        .get_user_send_as_document(chat_id.0)
                        .await
                        .map(|value| value == 1)
                        .unwrap_or(false)
                } else {
                    false
                };

                // Split video if Local Bot API is used and file exceeds 1.9GB
                let final_file_size = fs::metadata(&actual_file_path).map(|m| m.len()).unwrap_or(0);
                metrics::record_file_size("mp4", final_file_size);
                let is_local_bot_api = std::env::var("BOT_API_URL")
                    .map(|u| !u.contains("api.telegram.org"))
                    .unwrap_or(false);
                let target_part_size: u64 = 1900 * 1024 * 1024; // 1.9 GB

                let video_parts = if is_local_bot_api && final_file_size > target_part_size {
                    log::info!("Video > 1.9GB with Local Bot API — splitting into parts");
                    split_video_into_parts(&actual_file_path, target_part_size).await?
                } else {
                    vec![actual_file_path.clone()]
                };

                // ── Phase 3: Send each part ──
                let mut first_part_db_id = None;
                let total_parts = video_parts.len();

                for (idx, part_path) in video_parts.iter().enumerate() {
                    let part_index = (idx + 1) as i32;
                    let current_caption = if total_parts > 1 {
                        format!("{} (Part {}/{})", caption, part_index, total_parts)
                    } else {
                        caption.as_ref().to_string()
                    };

                    let (sent_message, file_size) = send_video_with_retry(
                        &bot_clone,
                        chat_id,
                        part_path,
                        &mut progress_msg,
                        &current_caption,
                        thumbnail_url.as_deref(),
                        send_as_document,
                        message_id,
                        Some(artist.clone()),
                    )
                    .await?;

                    // Save to download history
                    if let Some(ref storage) = shared_storage_clone {
                        let file_id = sent_message
                            .video()
                            .map(|v| v.file.id.0.clone())
                            .or_else(|| sent_message.document().map(|d| d.file.id.0.clone()));

                        let author_opt = if !artist.trim().is_empty() {
                            Some(artist.as_str())
                        } else {
                            None
                        };

                        let duration = probe_video_metadata(part_path).map(|(d, _, _)| d as i64);

                        match storage
                            .save_download_history(
                                chat_id.0,
                                url.as_str(),
                                title.as_str(),
                                "mp4",
                                file_id.as_deref(),
                                author_opt,
                                Some(file_size as i64),
                                duration,
                                Some(quality),
                                None,
                                first_part_db_id,
                                if total_parts > 1 { Some(part_index) } else { None },
                            )
                            .await
                        {
                            Ok(id) => {
                                let sent_msg_id = sent_message.id.0;
                                if let Err(e) = storage.update_download_message_id(id, sent_msg_id, chat_id.0).await {
                                    log::warn!("Failed to save message_id for download {}: {}", id, e);
                                }

                                // Save video timestamps (first part or single)
                                if total_parts == 1 || first_part_db_id.is_none() {
                                    if let Some(metadata) = PREVIEW_CACHE.get(url.as_str()).await {
                                        if !metadata.timestamps.is_empty() {
                                            // Filter timestamps to time range if download was clipped
                                            let ts_to_save = if let Some((ref start, ref end)) = *format.time_range() {
                                                use doracore::timestamps::{
                                                    filter_timestamps_for_range, parse_timestamp_to_secs,
                                                };
                                                match (parse_timestamp_to_secs(start), parse_timestamp_to_secs(end)) {
                                                    (Some(s), Some(e)) => {
                                                        filter_timestamps_for_range(&metadata.timestamps, s, e)
                                                    }
                                                    _ => metadata.timestamps.clone(),
                                                }
                                            } else {
                                                metadata.timestamps.clone()
                                            };
                                            if let Err(e) = storage.save_video_timestamps(id, &ts_to_save).await {
                                                log::warn!("Failed to save timestamps for download {}: {}", id, e);
                                            }
                                        }
                                    }
                                }

                                if first_part_db_id.is_none() && total_parts > 1 {
                                    first_part_db_id = Some(id);
                                }

                                // Add post-download buttons for single-part videos (not for time_range clips)
                                if total_parts == 1 && format.time_range().is_none() {
                                    let bot_for_button = bot_clone.clone();
                                    let msg_id = sent_message.id;
                                    let url_str = url.as_str().to_string();
                                    tokio::spawn(async move {
                                        use teloxide::types::InlineKeyboardMarkup;
                                        let mut rows = vec![vec![crate::telegram::cb(
                                            "✂️ Cut Video",
                                            format!("downloads:clip:{}", id),
                                        )]];
                                        // Add "Burn subtitles" for YouTube videos
                                        let is_yt = url_str.contains("://youtube.com/")
                                            || url_str.contains("://www.youtube.com/")
                                            || url_str.contains("://m.youtube.com/")
                                            || url_str.contains("://music.youtube.com/")
                                            || url_str.contains("://youtu.be/");
                                        if is_yt {
                                            rows.push(vec![crate::telegram::cb(
                                                "🔤 Burn subtitles",
                                                format!("downloads:burn_subs:{}", id),
                                            )]);
                                        }
                                        let keyboard = InlineKeyboardMarkup::new(rows);
                                        if let Err(e) = bot_for_button
                                            .edit_message_reply_markup(chat_id, msg_id)
                                            .reply_markup(keyboard)
                                            .await
                                        {
                                            log::warn!("Failed to add post-download buttons: {}", e);
                                        }
                                    });
                                }
                            }
                            Err(e) => {
                                log::warn!("Failed to save download history: {}", e);
                            }
                        }
                    }
                }

                // ── Phase 4: Success + cleanup ──
                let elapsed_secs = start_time.elapsed().as_secs();
                log::info!(
                    "⏱️ [VIDEO_TOTAL] done in {:.1}s (chat {})",
                    start_time.elapsed().as_secs_f64(),
                    chat_id.0
                );
                let _ = progress_msg
                    .update(
                        &bot_clone,
                        DownloadStatus::Success {
                            title: display_title.as_ref().to_string(),
                            elapsed_secs,
                            file_format: Some("mp4".to_string()),
                        },
                    )
                    .await;

                // Mark original message as completed
                if let Some(msg_id) = message_id {
                    use teloxide::types::MessageId;
                    crate::telegram::try_set_reaction(
                        &bot_clone,
                        chat_id,
                        MessageId(msg_id),
                        crate::telegram::success_reaction_for_format(Some("mp4")),
                    )
                    .await;
                }

                // Auto-clear success message after delay
                {
                    let bot_for_clear = bot_clone.clone();
                    let title_for_clear = Arc::clone(&display_title);
                    let mut msg_for_clear = progress_msg.clone_for_clear();
                    tokio::spawn(async move {
                        let _ = msg_for_clear
                            .clear_after(
                                &bot_for_clear,
                                config::progress::CLEAR_DELAY_SECS,
                                title_for_clear.as_ref().to_string(),
                                Some("mp4".to_string()),
                            )
                            .await;
                    });
                }

                // Share page: create after successful video send (YouTube only, fire-and-forget)
                if crate::core::share::is_youtube_url(url.as_str()) {
                    if let Some(ref storage) = shared_storage_clone {
                        let storage_share = Arc::clone(storage);
                        let url_str = url.to_string();
                        let title_share = title.clone();
                        let artist_share = artist.clone();
                        let duration_share = download_output.duration_secs;
                        let thumb_share = thumbnail_url.clone();
                        let bot_share = bot_clone.clone();
                        tokio::spawn(async move {
                            let thumb = thumb_share.or_else(|| crate::core::share::youtube_thumbnail_url(&url_str));
                            let artist_opt = if artist_share.trim().is_empty() {
                                None
                            } else {
                                Some(artist_share.as_str())
                            };
                            if let Some((share_url, streaming_links)) = crate::core::share::create_share_page(
                                &storage_share,
                                &url_str,
                                &title_share,
                                artist_opt,
                                thumb.as_deref(),
                                duration_share.map(|d| d as u64),
                            )
                            .await
                            {
                                send_share_message(
                                    &bot_share,
                                    chat_id,
                                    &title_share,
                                    &share_url,
                                    streaming_links.as_ref(),
                                )
                                .await;
                            }
                        });
                    }
                }

                // Schedule cleanup in background (outside timeout scope)
                {
                    let cleanup_paths: Vec<String> = {
                        let mut paths = Vec::new();
                        if total_parts > 1 {
                            paths.extend(video_parts.iter().cloned());
                        }
                        paths.push(actual_file_path.clone());
                        if actual_file_path != download_output.file_path {
                            paths.push(download_output.file_path.clone());
                        }
                        paths
                    };
                    tokio::spawn(async move {
                        tokio::time::sleep(config::download::cleanup_delay()).await;
                        for path in &cleanup_paths {
                            let _ = fs::remove_file(path);
                        }
                    });
                }

                Ok(())
            })
            .await
            {
                Ok(inner) => inner,
                Err(_) => {
                    log::error!(
                        "🚨 Video download timed out after {} seconds",
                        config::download::GLOBAL_TIMEOUT_SECS
                    );
                    Err(AppError::Download(DownloadError::Timeout(format!(
                        "Video upload timeout (exceeded {} minutes)",
                        config::download::GLOBAL_TIMEOUT_SECS / 60
                    ))))
                }
            };

            match result {
                Ok(()) => {
                    log::info!("Video download completed successfully for chat {}", chat_id);
                    timer.observe_duration();
                    metrics::record_download_success("mp4", quality);
                    let signoff = crate::i18n::random_signoff(&lang);
                    let _ = bot_clone
                        .send_message(chat_id, signoff)
                        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                        .await;
                }
                Err(e) => {
                    e.track_with_operation("video_download");
                    log::error!("Video download error for chat {} ({}): {:?}", chat_id, url, e);
                    timer.observe_duration();

                    // Delete hanging ⏳ progress message so it doesn't stay on screen forever
                    if let Some(msg_id) = progress_msg.message_id {
                        let _ = bot_clone.delete_message(chat_id, msg_id).await;
                    }

                    let pipeline_error = pipeline::PipelineError::Operational(e);
                    pipeline::handle_pipeline_error(
                        &bot_clone,
                        chat_id,
                        &url,
                        &pipeline_error,
                        &format,
                        alert_manager.as_ref(),
                        message_id,
                    )
                    .await;
                }
            }
        }
        .instrument(span),
    );
    Ok(())
}

/// Get thumbnail URL from yt-dlp (best-effort, non-blocking).
async fn get_thumbnail_url(url: &Url) -> Option<String> {
    let ytdl_bin = &*config::YTDL_BIN;
    let mut args: Vec<&str> = vec![
        "--get-thumbnail",
        "--no-playlist",
        "--socket-timeout",
        "30",
        "--retries",
        "2",
    ];
    add_cookies_args(&mut args);
    let url_str = url.as_str();
    args.push(url_str);

    let result = timeout(
        config::download::ytdlp_timeout(),
        TokioCommand::new(ytdl_bin).args(&args).output(),
    )
    .await
    .ok()?;

    let output = result.ok()?;
    if output.status.success() {
        let thumb_url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if thumb_url.is_empty() {
            None
        } else {
            Some(thumb_url)
        }
    } else {
        None
    }
}

/// Download dubbed audio track separately and merge it into the video via ffmpeg.
///
/// Uses yt-dlp `--extract-audio` with `player_client=android,default` + language filter.
/// The android client sees dubbed tracks; cookies + PO token handle authentication.
/// Falls back to original audio if download fails.
async fn replace_audio_track(video_path: &str, url: &Url, lang: &str) -> String {
    use crate::download::metadata::{add_cookies_args_with_proxy, add_no_cookies_args, get_proxy_chain};

    log::info!("🔊 Downloading dubbed audio (lang={}) for {}", lang, url);

    let ytdl_bin = config::YTDL_BIN.clone();
    let audio_output = format!("{}.dubbed_audio.m4a", video_path);
    let format_filter = format!("ba[language={}]/ba[language^={}]", lang, lang);
    let url_str = url.to_string();
    let video_path_owned = video_path.to_string();

    // Use spawn_blocking for the yt-dlp download (same pattern as main download)
    let handle = tokio::task::spawn_blocking(move || {
        let proxy_chain = get_proxy_chain();

        for (attempt, proxy_option) in proxy_chain.iter().enumerate() {
            let proxy_name = proxy_option
                .as_ref()
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "Direct".to_string());

            // ── Tier 1: no cookies ──
            {
                let mut args: Vec<&str> = vec![
                    "--no-playlist",
                    "--extract-audio",
                    "--audio-format",
                    "m4a",
                    "--format",
                    &format_filter,
                    "--output",
                    &audio_output,
                    "--no-check-certificate",
                    "--age-limit",
                    "99",
                ];
                add_no_cookies_args(&mut args, proxy_option.as_ref());
                args.extend_from_slice(&[
                    "--extractor-args",
                    "youtube:player_client=android,default;formats=missing_pot",
                    "--js-runtimes",
                    "deno",
                    "--impersonate",
                    "Chrome-131:Android-14",
                ]);
                args.push(&url_str);

                log::info!("🔊 Audio track tier1 attempt {}, proxy [{}]", attempt + 1, proxy_name);

                let output = std::process::Command::new(&*ytdl_bin).args(&args).output();

                match output {
                    Ok(o) if o.status.success() => {
                        log::info!("🔊 Dubbed audio downloaded (tier1)");
                        return Ok(audio_output);
                    }
                    Ok(o) => {
                        let stderr = String::from_utf8_lossy(&o.stderr);
                        log::warn!("🔊 Tier1 failed: {}", &stderr[..stderr.len().min(200)]);
                    }
                    Err(e) => log::warn!("🔊 Tier1 exec error: {}", e),
                }
            }

            // ── Tier 2: cookies + PO token ──
            {
                let mut args: Vec<&str> = vec![
                    "--no-playlist",
                    "--extract-audio",
                    "--audio-format",
                    "m4a",
                    "--format",
                    &format_filter,
                    "--output",
                    &audio_output,
                    "--no-check-certificate",
                    "--age-limit",
                    "99",
                ];
                add_cookies_args_with_proxy(&mut args, proxy_option.as_ref());
                args.extend_from_slice(&[
                    "--extractor-args",
                    "youtube:player_client=android,default;formats=missing_pot",
                    "--js-runtimes",
                    "deno",
                ]);
                args.push(&url_str);

                log::info!("🔊 Audio track tier2 attempt {}, proxy [{}]", attempt + 1, proxy_name);

                let output = std::process::Command::new(&*ytdl_bin).args(&args).output();

                match output {
                    Ok(o) if o.status.success() => {
                        log::info!("🔊 Dubbed audio downloaded (tier2)");
                        return Ok(audio_output);
                    }
                    Ok(o) => {
                        let stderr = String::from_utf8_lossy(&o.stderr);
                        log::warn!("🔊 Tier2 failed: {}", &stderr[..stderr.len().min(200)]);
                    }
                    Err(e) => log::warn!("🔊 Tier2 exec error: {}", e),
                }
            }
        }

        Err("All tiers/proxies failed for dubbed audio download".to_string())
    });

    // Timeout: max 3 minutes for dubbed audio download
    let download_result = match timeout(std::time::Duration::from_secs(180), handle).await {
        Ok(join_result) => join_result.unwrap_or_else(|e| Err(format!("spawn_blocking error: {}", e))),
        Err(_) => {
            log::error!("🔊 Dubbed audio download timed out (180s)");
            Err("Timeout".to_string())
        }
    };

    let audio_path = match download_result {
        Ok(path) => path,
        Err(e) => {
            log::error!("🔊 Failed to download dubbed audio: {}", e);
            return video_path.to_string();
        }
    };

    // Find the actual audio file (yt-dlp may change extension)
    let actual_audio = if std::path::Path::new(&audio_path).exists() {
        audio_path.clone()
    } else {
        // Look for files matching the pattern
        let parent = std::path::Path::new(&audio_path)
            .parent()
            .unwrap_or(std::path::Path::new("."));
        let found = std::fs::read_dir(parent)
            .ok()
            .and_then(|entries| {
                entries.flatten().find(|e| {
                    e.path().to_string_lossy().contains(".dubbed_audio.")
                        && e.path().to_string_lossy().starts_with(&video_path_owned)
                })
            })
            .map(|e| e.path().to_string_lossy().to_string());

        match found {
            Some(p) => {
                log::info!("🔊 Found audio at: {}", p);
                p
            }
            None => {
                log::error!("🔊 Audio file not found after download");
                return video_path.to_string();
            }
        }
    };

    // ffmpeg: replace audio in video
    let merged_path = format!("{}.merged.mp4", video_path);
    log::info!("🔊 Merging dubbed audio into video...");

    let ffmpeg_result = TokioCommand::new("ffmpeg")
        .args([
            "-y",
            "-i",
            video_path,
            "-i",
            &actual_audio,
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-movflags",
            "+faststart",
            &merged_path,
        ])
        .output()
        .await;

    match ffmpeg_result {
        Ok(output) if output.status.success() => {
            log::info!("🔊 Audio replaced successfully");
            // Replace original with merged
            if std::fs::rename(&merged_path, video_path).is_err() {
                if let Err(e) = std::fs::copy(&merged_path, video_path) {
                    log::error!("🔊 Failed to copy merged file: {}", e);
                }
                if let Err(e) = std::fs::remove_file(&merged_path) {
                    log::warn!("🔊 Failed to clean up merged file: {}", e);
                }
            }
            let _ = std::fs::remove_file(&actual_audio);
            video_path.to_string()
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            log::error!("🔊 ffmpeg failed: {}", &stderr[..stderr.len().min(300)]);
            let _ = std::fs::remove_file(&merged_path);
            let _ = std::fs::remove_file(&actual_audio);
            video_path.to_string()
        }
        Err(e) => {
            log::error!("🔊 ffmpeg exec error: {}", e);
            let _ = std::fs::remove_file(&actual_audio);
            video_path.to_string()
        }
    }
}

/// Burn subtitles into video if requested.
///
/// Checks (in order):
/// 1. Per-URL cached language from the preview "Burn subtitles" button
/// 2. User DB settings (download_subtitles + burn_subtitles flags with "en,ru")
///
/// Returns the final file path (with or without burned subtitles).
async fn maybe_burn_subtitles(
    file_path: &str,
    url: &Url,
    shared_storage: Option<&Arc<SharedStorage>>,
    chat_id: ChatId,
) -> String {
    // Check for per-URL burn subtitle language (from preview button)
    let cached_lang = if let Some(storage) = shared_storage {
        storage
            .get_preview_context(chat_id.0, url.as_str())
            .await
            .ok()
            .flatten()
            .and_then(|context| context.burn_sub_lang)
    } else {
        None
    };

    let sub_lang = if let Some(lang) = cached_lang {
        log::info!("Using cached burn subtitle language '{}' for {}", lang, url.as_str());
        lang
    } else {
        // Fall through to existing DB settings check
        let Some(storage) = shared_storage else {
            return file_path.to_string();
        };
        let download_subs = storage.get_user_download_subtitles(chat_id.0).await.unwrap_or(false);
        let burn_subs = storage.get_user_burn_subtitles(chat_id.0).await.unwrap_or(false);

        if !(download_subs && burn_subs) {
            return file_path.to_string();
        }

        log::info!("User DB settings request burned subtitles (en,ru)");
        "en,ru".to_string()
    };

    // Fetch subtitle style from DB (use default if no pool or error)
    let subtitle_style = if let Some(storage) = shared_storage {
        storage.get_user_subtitle_style(chat_id.0).await.unwrap_or_default()
    } else {
        Default::default()
    };

    let safe_base = std::path::Path::new(file_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("video");

    let download_folder = shellexpand::tilde(&*config::DOWNLOAD_FOLDER).into_owned();
    // Don't add .srt extension — yt-dlp adds .{lang}.srt automatically
    let subtitle_path = format!("{}/{}_subs", download_folder, safe_base);

    let ytdl_bin = &*config::YTDL_BIN;
    let mut subtitle_args: Vec<&str> = vec![
        "--write-subs",
        "--write-auto-subs",
        "--sub-lang",
        &sub_lang,
        "--sub-format",
        "srt",
        "--convert-subs",
        "srt",
        "--skip-download",
        "--output",
        &subtitle_path,
        "--no-playlist",
    ];
    add_cookies_args(&mut subtitle_args);
    subtitle_args.push(url.as_str());

    let mut sub_cmd = TokioCommand::new(ytdl_bin);
    sub_cmd.args(&subtitle_args);
    let subtitle_output = run_with_timeout(&mut sub_cmd, config::download::ytdlp_timeout()).await;

    match subtitle_output {
        Ok(output) if output.status.success() => {
            // Find actual subtitle file (yt-dlp may add language suffix)
            let subtitle_file = std::fs::read_dir(&download_folder).ok().and_then(|entries| {
                entries
                    .filter_map(Result::ok)
                    .find(|entry| {
                        let name = entry.file_name();
                        let name_str = name.to_string_lossy();
                        name_str.contains(safe_base) && name_str.ends_with(".srt")
                    })
                    .map(|entry| entry.path().display().to_string())
            });

            if let Some(sub_file) = subtitle_file {
                log::info!("Subtitles downloaded: {}", sub_file);

                let output_with_subs = format!("{}_with_subs.mp4", file_path.trim_end_matches(".mp4"));

                match burn_subtitles_into_video(file_path, &sub_file, &output_with_subs, &subtitle_style).await {
                    Ok(_) => {
                        log::info!("Successfully burned subtitles into video");
                        let _ = std::fs::remove_file(file_path);
                        let _ = std::fs::remove_file(&sub_file);
                        return output_with_subs;
                    }
                    Err(e) => {
                        log::error!("Failed to burn subtitles: {}. Using original.", e);
                        let _ = std::fs::remove_file(&sub_file);
                    }
                }
            } else {
                log::warn!("Subtitle file not found after download");
            }
        }
        Ok(output) => {
            log::warn!(
                "yt-dlp subtitle download failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Err(e) => {
            log::warn!("Failed to execute yt-dlp for subtitles: {}", e);
        }
    }

    file_path.to_string()
}

/// Send a follow-up Telegram message with streaming service buttons after a successful download.
async fn send_share_message(
    bot: &Bot,
    chat_id: ChatId,
    title: &str,
    share_url: &str,
    streaming_links: Option<&crate::core::odesli::StreamingLinks>,
) {
    use teloxide::requests::Requester;
    use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

    let mut row1: Vec<InlineKeyboardButton> = Vec::new();
    let mut has_links = false;

    if let Some(links) = streaming_links {
        if let Some(ref url) = links.spotify {
            if let Ok(u) = url.parse() {
                row1.push(InlineKeyboardButton::url("💚 Spotify", u));
                has_links = true;
            }
        }
        if let Some(ref url) = links.apple_music {
            if let Ok(u) = url.parse() {
                row1.push(InlineKeyboardButton::url("🍎 Apple", u));
                has_links = true;
            }
        }
        if let Some(ref url) = links.youtube_music {
            if let Ok(u) = url.parse() {
                row1.push(InlineKeyboardButton::url("🔴 YT Music", u));
                has_links = true;
            }
        }
    }

    let Ok(share_parsed) = share_url.parse() else {
        log::warn!("Invalid share URL: {}", share_url);
        return;
    };
    let row2 = vec![InlineKeyboardButton::url("🔗 All platforms", share_parsed)];

    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    if !row1.is_empty() {
        rows.push(row1);
    }
    rows.push(row2);

    let keyboard = InlineKeyboardMarkup::new(rows);

    let text = if has_links {
        format!("🎧 \"{}\" — listen legally:", title)
    } else {
        format!("🔗 \"{}\":", title)
    };

    if let Err(e) = bot.send_message(chat_id, text).reply_markup(keyboard).await {
        log::warn!("Failed to send share message: {}", e);
    }
}
