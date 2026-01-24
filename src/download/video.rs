//! Video download and processing module
//!
//! This module handles downloading video files from URLs using yt-dlp,
//! tracking progress, and sending them to Telegram users.

use crate::core::config;
use crate::core::error::AppError;
use crate::core::metrics;
use crate::core::truncate_tail_utf8;
use crate::download::downloader::{parse_progress, ProgressInfo};
use crate::download::metadata::add_cookies_args;
use crate::download::ytdlp_errors::{analyze_ytdlp_error, get_error_message, should_notify_admin, YtDlpErrorType};
use crate::telegram::notifications::notify_admin_text;
use crate::telegram::Bot;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::Arc;
use teloxide::prelude::*;
use url::Url;

/// Downloads video with real-time progress tracking via channel
///
/// Returns a receiver for progress updates and a join handle for the download task.
/// The download runs in a blocking task to read stdout line by line.
pub async fn download_video_file_with_progress(
    admin_bot: Bot,
    user_chat_id: ChatId,
    url: &Url,
    download_path: &str,
    format_arg: &str,
) -> Result<
    (
        tokio::sync::mpsc::UnboundedReceiver<ProgressInfo>,
        tokio::task::JoinHandle<Result<(), AppError>>,
    ),
    AppError,
> {
    let ytdl_bin = config::YTDL_BIN.clone();
    let url_str = url.to_string();
    let download_path_clone = download_path.to_string();
    let format_arg_clone = format_arg.to_string();
    let runtime_handle = tokio::runtime::Handle::current();

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    let handle = tokio::task::spawn_blocking(move || {
        let mut args: Vec<&str> = vec![
            "-o",
            &download_path_clone,
            "--newline",
            "--format",
            &format_arg_clone,
            "--merge-output-format",
            "mp4",
            "--concurrent-fragments",
            "3",
            "--fragment-retries",
            "10",
            "--socket-timeout",
            "30",
            "--http-chunk-size",
            "10485760",
            "--sleep-requests",
            "1",
            "--postprocessor-args",
            "ffmpeg:-movflags +faststart",
        ];
        add_cookies_args(&mut args);

        // Use default web client with cookies (not Android which requires PO Token)
        args.push("--extractor-args");
        args.push("youtube:player_client=default,web_safari,web_embedded");

        args.extend_from_slice(&["--no-check-certificate", &url_str]);

        let command_str = format!("{} {}", ytdl_bin, args.join(" "));
        log::info!("[DEBUG] yt-dlp command for video download: {}", command_str);

        let mut child = Command::new(&ytdl_bin)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| AppError::Download(format!("Failed to spawn yt-dlp: {}", e)))?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let stderr_lines = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let stdout_lines = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));

        use std::thread;
        let tx_clone = tx.clone();
        let stderr_lines_clone = Arc::clone(&stderr_lines);
        let stdout_lines_clone = Arc::clone(&stdout_lines);

        if let Some(stderr_stream) = stderr {
            thread::spawn(move || {
                let reader = BufReader::new(stderr_stream);
                for line in reader.lines() {
                    if let Ok(line_str) = line {
                        log::debug!("yt-dlp stderr: {}", line_str);
                        if let Ok(mut lines) = stderr_lines_clone.lock() {
                            lines.push(line_str.clone());
                            if lines.len() > 200 {
                                lines.remove(0);
                            }
                        }
                        if let Some(progress_info) = parse_progress(&line_str) {
                            log::info!("Parsed progress from stderr: {}%", progress_info.percent);
                            let _ = tx_clone.send(progress_info);
                        }
                    }
                }
            });
        }

        if let Some(stdout_stream) = stdout {
            let reader = BufReader::new(stdout_stream);
            for line in reader.lines() {
                if let Ok(line_str) = line {
                    log::debug!("yt-dlp stdout: {}", line_str);
                    if let Ok(mut lines) = stdout_lines_clone.lock() {
                        lines.push(line_str.clone());
                        if lines.len() > 200 {
                            lines.remove(0);
                        }
                    }
                    if let Some(progress_info) = parse_progress(&line_str) {
                        let _ = tx.send(progress_info);
                    }
                }
            }
        }

        let status = child
            .wait()
            .map_err(|e| AppError::Download(format!("downloader process failed: {}", e)))?;

        if !status.success() {
            let stderr_text = if let Ok(lines) = stderr_lines.lock() {
                lines.join("\n")
            } else {
                String::new()
            };
            let stdout_text = if let Ok(lines) = stdout_lines.lock() {
                lines.join("\n")
            } else {
                String::new()
            };

            if !stderr_text.is_empty() {
                let error_type = analyze_ytdlp_error(&stderr_text);

                let error_category = match error_type {
                    YtDlpErrorType::InvalidCookies => "invalid_cookies",
                    YtDlpErrorType::BotDetection => "bot_detection",
                    YtDlpErrorType::VideoUnavailable => "video_unavailable",
                    YtDlpErrorType::NetworkError => "network",
                    YtDlpErrorType::FragmentError => "fragment_error",
                    YtDlpErrorType::Unknown => "ytdlp_unknown",
                };
                let operation = format!("video_download:{}", error_category);
                metrics::record_error("download", &operation);

                log::error!("yt-dlp download failed, error type: {:?}", error_type);
                log::error!("yt-dlp stderr: {}", stderr_text);

                if should_notify_admin(&error_type) {
                    log::warn!("This error requires administrator attention!");
                    let admin_message = format!(
                        "YTDLP ERROR (video download)\nuser_chat_id: {}\nurl: {}\nerror_type: {:?}\n\ncommand:\n{}\n\nstdout (tail):\n{}\n\nstderr (tail):\n{}",
                        user_chat_id.0,
                        url_str,
                        error_type,
                        command_str,
                        truncate_tail_utf8(&stdout_text, 6000),
                        truncate_tail_utf8(&stderr_text, 6000),
                    );
                    let bot_for_admin = admin_bot.clone();
                    runtime_handle.spawn(async move {
                        notify_admin_text(&bot_for_admin, &admin_message).await;
                    });
                }

                return Err(AppError::Download(get_error_message(&error_type)));
            } else {
                metrics::record_error("download", "video_download");
                return Err(AppError::Download(format!("downloader exited with status: {}", status)));
            }
        }

        Ok(())
    });

    Ok((rx, handle))
}

// Note: download_and_send_video remains in downloader.rs for now due to its complexity
// and dependencies on many internal functions. It calls download_video_file_with_progress
// from this module.
