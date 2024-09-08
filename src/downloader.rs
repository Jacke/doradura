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
use std::process::Command;
use chrono::{DateTime, Utc};

#[derive(Error, Debug)]
pub enum CommandError {
    #[error("Failed to fetch song metadata")]
    FetchMetadata(#[from] Error),
    #[error("Failed to download file")]
    Download(Error),
}

pub async fn download_and_send_audio(bot: Bot, msg: Message, url: Url, rate_limiter: Arc<RateLimiter>, created_timestamp: DateTime<Utc>) -> ResponseResult<()> {
    let bot_clone = bot.clone();
    let chat_id = msg.chat.id;
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
            
            let mut child = Command::new("youtube-dl")
                .arg("-o")
                .arg(&download_path)
                .arg("--verbose")
                .arg("--extract-audio")
                .arg("--audio-format")
                .arg("mp3")
                .arg("--audio-quality")
                .arg("0")
                .arg("--add-metadata")
                .arg("--embed-thumbnail")
                .arg("--no-playlist")
                .arg("--postprocessor-args")
                .arg("-acodec libmp3lame -b:a 320k") // Set audio codec to libmp3lame and bitrate to 320 kbps 
                .arg(url.as_str())
                .spawn()
                .map_err(|e| CommandError::Download(anyhow!("Failed to start youtube-dl process: {}", e)))?;
            let _ = child.wait().map_err(|e| CommandError::Download(anyhow!("youtube-dl process failed: {}", e)))?;

            println!("download_path {:?}", download_path);

            // Use ffprobe to get the duration of the audio file
            let output = Command::new("ffprobe")
                .args(&[
                    "-v", "error",
                    "-show_entries", "format=duration",
                    "-of", "default=noprint_wrappers=1:nokey=1",
                    &download_path,
                ])
                .output()
                .map_err(|e| CommandError::Download(anyhow!("Failed to execute ffprobe: {}", e)))?;
            let duration_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let duration: u32 = duration_str.parse::<f32>().unwrap_or(0.0).round() as u32;

            // Calculate and print the elapsed time
            let current_time = Utc::now();
            let elapsed_time = current_time.signed_duration_since(created_timestamp);
            println!("Elapsed time for audio download: {:?}", elapsed_time);
            println!("Audio has been downloaded path: {:?} duration: {:?}", download_path, duration_str); 
            
            bot_clone
                .send_audio(chat_id, InputFile::file(&download_path))
                .duration(duration)
                .await
                .map_err(|e| CommandError::Download(anyhow!("Failed to send audio file: {}", e)))?;
            
            bot_clone
                .send_audio(chat_id, InputFile::file(&download_path))
                .duration(duration)
                .await
                .map_err(|e| CommandError::Download(anyhow!("Failed to send audio file: {}", e)))?;

                match send_audio_with_retry(&bot, chat_id, &download_path, duration).await {
                    Ok(_) => log::info!("Audio sent successfully."),
                    Err(e) => log::error!("Error sending audio: {}", e),
                }      
                
            tokio::time::sleep(Duration::from_secs(600)).await;
            std::fs::remove_file(&download_path).map_err(|e| CommandError::Download(anyhow!("Failed to delete file: {}", e)))?;

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
    let mut attempts = 0;
    let max_attempts = 3;

    loop {
        let response = bot.send_audio(chat_id, InputFile::file(download_path))
            .duration(duration)
            .await;  // Manually await each call

        match response {
            Ok(_) => return Ok(()),
            Err(e) if attempts < max_attempts => {
                log::warn!("Attempt {} failed, error: {}. Retrying...", attempts + 1, e);
                attempts += 1;
                tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
            },
            Err(e) => return Err(CommandError::Download(anyhow!("Failed to send audio file: {}", e.to_string()))),
        }
    }
}

pub async fn download_and_send_video(bot: Bot, msg: Message, url: Url, rate_limiter: Arc<RateLimiter>, created_timestamp: DateTime<Utc>) -> ResponseResult<()> {
    let bot_clone = bot.clone();
    let chat_id = msg.chat.id;
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
            
            let mut child = Command::new("youtube-dl")
                .arg("-o")
                .arg(&download_path)
                .arg("--verbose")
                .arg("--format")
                .arg("best")
                .arg(url.as_str())
                .spawn()
                .expect("Failed to start youtube-dl process");
            let _ = child.wait().expect("youtube-dl process failed");

            println!("download_path {:?}", download_path);
            // Calculate and print the elapsed time
            let current_time = Utc::now();
            let elapsed_time = current_time.signed_duration_since(created_timestamp);
            println!("Elapsed time for video download: {:?}", elapsed_time);


            bot_clone
                .send_video(chat_id, InputFile::file(&download_path))
                .await
                .map_err(|e| CommandError::Download(anyhow!("У меня не получилось отправить тебе видео 🥲 попробуй как-нибудь позже, вот что мне показывает: {}", e)))?;
                
            tokio::time::sleep(Duration::from_secs(600)).await;
            std::fs::remove_file(&download_path).expect("Failed to delete file");

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
