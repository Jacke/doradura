use teloxide::prelude::*;
use teloxide::types::InputFile;
use crate::rate_limiter::RateLimiter;
use crate::fetch::fetch_song_metadata;
use crate::utils::escape_filename;
use std::sync::Arc;
use std::time::Duration;
use url::Url;
use thiserror::Error;
use anyhow::{Error, anyhow};
use std::process::{Command, Stdio};
use chrono::{DateTime, Utc};
use std::env;
use std::fs;

#[derive(Error, Debug)]
pub enum CommandError {
    #[error("Failed to fetch song metadata")]
    FetchMetadata(#[from] Error),
    #[error("Failed to download file")]
    Download(Error),
}

fn probe_duration_seconds(path: &str) -> Option<u32> {
    let output = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-show_entries", "format=duration",
            "-of", "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
        .ok()?;

    let duration_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if duration_str.is_empty() { return None; }
    let secs = duration_str.parse::<f32>().ok()?;
    Some(secs.round() as u32)
}

fn spawn_downloader_with_fallback(ytdl_bin: &str, args: &[&str]) -> Result<std::process::Child, CommandError> {
    Command::new(ytdl_bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .or_else(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                let fallback = "youtube-dl";
                Command::new(fallback)
                    .args(args)
                    .stdin(Stdio::null())
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .map_err(|inner| CommandError::Download(anyhow!(
                        "Failed to start downloader. Tried '{}', then '{}': {} / {}",
                        ytdl_bin, fallback, e, inner
                    )))
            } else {
                Err(CommandError::Download(anyhow!("Failed to start downloader '{}': {}", ytdl_bin, e)))
            }
        })
}

fn download_audio_file(url: &Url, download_path: &str) -> Result<Option<u32>, CommandError> {
    let ytdl_bin = env::var("YTDL_BIN").ok().unwrap_or_else(|| "yt-dlp".to_string());
    let args = [
        "-o", download_path,
        "--verbose",
        "--extract-audio",
        "--audio-format", "mp3",
        "--audio-quality", "0",
        "--add-metadata",
        "--embed-thumbnail",
        "--no-playlist",
        "--concurrent-fragments", "5", // Parallel download of fragments for faster downloads
        "--postprocessor-args", "-acodec libmp3lame -b:a 320k",
        url.as_str(),
    ];
    let mut child = spawn_downloader_with_fallback(&ytdl_bin, &args)?;
    let status = child
        .wait()
        .map_err(|e| CommandError::Download(anyhow!("downloader process failed: {}", e)))?;
    if !status.success() {
        return Err(CommandError::Download(anyhow!("downloader exited with status: {}", status)));
    }
    Ok(probe_duration_seconds(download_path))
}

pub async fn download_and_send_audio(bot: Bot, chat_id: ChatId, url: Url, rate_limiter: Arc<RateLimiter>, created_timestamp: DateTime<Utc>) -> ResponseResult<()> {
    let bot_clone = bot.clone();
    let _rate_limiter = Arc::clone(&rate_limiter);

    tokio::spawn(async move {
        let result: Result<(), CommandError> = async {
            let (title, artist) = fetch_song_metadata(&url.as_str())
                .await
                .map_err(|e| CommandError::FetchMetadata(anyhow!("Failed to fetch song metadata: {}", e)))?;
            let file_name = generate_file_name(&title, &artist);
            let safe_filename = escape_filename(&file_name);
            let full_path = format!("~/downloads/{}", safe_filename);
            let download_path = shellexpand::tilde(&full_path).into_owned();
            
            let duration = download_audio_file(&url, &download_path)?;

            println!("download_path {:?}", download_path);

            let duration: u32 = duration.unwrap_or(0);

            // Calculate and print the elapsed time
            let current_time = Utc::now();
            let elapsed_time = current_time.signed_duration_since(created_timestamp);
            println!("Elapsed time for audio download: {:?}", elapsed_time);
            println!("Audio has been downloaded path: {:?} duration_secs: {:?}", download_path, duration);
            
            // Send audio with retry logic
            match send_audio_with_retry(&bot_clone, chat_id, &download_path, duration).await {
                Ok(_) => {
                    println!("Audio sent successfully to chat {}", chat_id);
                },
                Err(e) => {
                    println!("Error sending audio: {}", e);
                    return Err(e);
                },
            }
                
            tokio::time::sleep(Duration::from_secs(600)).await;
            fs::remove_file(&download_path).map_err(|e| CommandError::Download(anyhow!("Failed to delete file: {}", e)))?;

            Ok(())
        }.await;

        if let Err(e) = result {
            println!("An error occurred: {:?}", e);
            bot_clone
                .send_message(chat_id, format!("An error occurred: {}", e.to_string()))
                .await
                .unwrap();
        }
    });
    Ok(())
}

async fn send_audio_with_retry(bot: &Bot, chat_id: ChatId, download_path: &str, duration: u32) -> Result<(), CommandError> {
    let max_attempts = 3;

    for attempt in 1..=max_attempts {
        log::info!("Attempting to send audio to chat {} (attempt {}/{})", chat_id, attempt, max_attempts);

        let response = bot.send_audio(chat_id, InputFile::file(download_path))
            .duration(duration)
            .await;

        match response {
            Ok(_) => {
                log::info!("Successfully sent audio to chat {} on attempt {}", chat_id, attempt);
                return Ok(());
            },
            Err(e) if attempt < max_attempts => {
                log::warn!("Attempt {}/{} failed for chat {}: {}. Retrying in 10 seconds...",
                    attempt, max_attempts, chat_id, e);
                tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
            },
            Err(e) => {
                log::error!("All {} attempts failed to send audio to chat {}: {}", max_attempts, chat_id, e);
                return Err(CommandError::Download(anyhow!("Failed to send audio file after {} attempts: {}", max_attempts, e.to_string())));
            },
        }
    }

    unreachable!()
}

async fn send_video_with_retry(bot: &Bot, chat_id: ChatId, download_path: &str) -> Result<(), CommandError> {
    let max_attempts = 3;

    for attempt in 1..=max_attempts {
        log::info!("Attempting to send video to chat {} (attempt {}/{})", chat_id, attempt, max_attempts);

        let response = bot.send_video(chat_id, InputFile::file(download_path))
            .await;

        match response {
            Ok(_) => {
                log::info!("Successfully sent video to chat {} on attempt {}", chat_id, attempt);
                return Ok(());
            },
            Err(e) if attempt < max_attempts => {
                log::warn!("Attempt {}/{} failed for chat {}: {}. Retrying in 10 seconds...",
                    attempt, max_attempts, chat_id, e);
                tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
            },
            Err(e) => {
                log::error!("All {} attempts failed to send video to chat {}: {}", max_attempts, chat_id, e);
                return Err(CommandError::Download(anyhow!("У меня не получилось отправить тебе видео 🥲 попробуй как-нибудь позже. Все {} попытки не удались: {}", max_attempts, e)));
            },
        }
    }

    unreachable!()
}

pub async fn download_and_send_video(bot: Bot, chat_id: ChatId, url: Url, rate_limiter: Arc<RateLimiter>, created_timestamp: DateTime<Utc>) -> ResponseResult<()> {
    let bot_clone = bot.clone();
    let _rate_limiter = Arc::clone(&rate_limiter);

    tokio::spawn(async move {
        let result: Result<(), CommandError> = async {
            let (title, artist) = fetch_song_metadata(&url.as_str())
                .await
                .map_err(|e| CommandError::FetchMetadata(anyhow!("Failed to fetch video metadata: {}", e)))?;
            let file_name = generate_file_name(&title, &artist);
            let safe_filename = escape_filename(&file_name);
            let full_path = format!("~/downloads/{}", safe_filename);
            let download_path = shellexpand::tilde(&full_path).into_owned();

            let ytdl_bin = env::var("YTDL_BIN").ok().unwrap_or_else(|| "yt-dlp".to_string());
            let args = [
                "-o", &download_path,
                "--verbose",
                "--format", "best",
                "--concurrent-fragments", "5", // Parallel download of fragments for faster downloads
                url.as_str(),
            ];
            let mut child = spawn_downloader_with_fallback(&ytdl_bin, &args)?;
            let status = child.wait().map_err(|e| CommandError::Download(anyhow!("downloader process failed: {}", e)))?;

            if !status.success() {
                return Err(CommandError::Download(anyhow!("downloader exited with status: {}", status)));
            }

            println!("download_path {:?}", download_path);
            // Calculate and print the elapsed time
            let current_time = Utc::now();
            let elapsed_time = current_time.signed_duration_since(created_timestamp);
            println!("Elapsed time for video download: {:?}", elapsed_time);

            // Send video with retry logic
            send_video_with_retry(&bot_clone, chat_id, &download_path).await?;

            tokio::time::sleep(Duration::from_secs(600)).await;
            fs::remove_file(&download_path).map_err(|e| CommandError::Download(anyhow!("Failed to delete file: {}", e)))?;

            Ok(())
        }.await;

        if let Err(e) = result {
            bot_clone
                .send_message(chat_id, format!("An error occurred: {}", e))
                .await
                .unwrap();
        }
    });
    Ok(())
}

fn generate_file_name(title: &str, artist: &str) -> String {
    if artist.trim().is_empty() && title.trim().is_empty() {
        "Unknown.mp3".to_string()
    } else if artist.trim().is_empty() {
        format!("{}.mp3", title)
    } else if title.trim().is_empty() {
        format!("{}.mp3", artist)
    } else {
        format!("{} - {}.mp3", artist, title)
    }
}

#[cfg(test)]
mod download_tests {
    use super::*;

    fn tool_exists(bin: &str) -> bool {
        Command::new("which").arg(bin).output().map(|o| o.status.success()).unwrap_or(false)
    }

    #[test]
    fn test_probe_duration_seconds_handles_missing_file() {
        assert_eq!(probe_duration_seconds("/no/such/file.mp3"), None);
    }

    #[test]
    fn test_spawn_downloader_fails_without_tools() {
        if tool_exists("yt-dlp") || tool_exists("youtube-dl") {
            // Tools present; skip this specific negative test.
            return;
        }
        let res = spawn_downloader_with_fallback("youtube-dl", &["--version"]);
        assert!(res.is_err());
    }

    // Integration-ish test: requires network and yt-dlp (or youtube-dl) + ffmpeg installed.
    // It downloads to a temp path and ensures file appears, then cleans up.
    #[test]
    #[ignore]
    fn test_download_audio_file_from_youtube() {
        if !(tool_exists("yt-dlp") || tool_exists("youtube-dl")) {
            eprintln!("skipping: no yt-dlp/youtube-dl in PATH");
            return;
        }
        if !tool_exists("ffprobe") { // ffmpeg suite
            eprintln!("skipping: no ffprobe in PATH");
            return;
        }
        let url = Url::parse("https://www.youtube.com/watch?v=0CAltmPaNZY").unwrap();
        let tmp_dir = std::env::temp_dir();
        let dest = tmp_dir.join(format!("test_dl_{}.mp3", uuid::Uuid::new_v4()));
        let dest_str = dest.to_string_lossy().to_string();
        let res = download_audio_file(&url, &dest_str);
        match res {
            Ok(_dur_opt) => {
                assert!(std::path::Path::new(&dest_str).exists());
                let _ = fs::remove_file(&dest_str);
            }
            Err(e) => panic!("download failed: {:?}", e),
        }
    }
}
