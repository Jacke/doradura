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
// use ffmpeg_next as ffmpeg;
use reqwest::Client;
use crate::get_updates_with_retry;

#[derive(Error, Debug)]
enum CommandError {
    #[error("Failed to fetch song metadata")]
    FetchMetadata(#[from] Error),
    #[error("Failed to download file")]
    Download(Error),
}

pub async fn handle_rate_limit(bot: &Bot, msg: &Message, rate_limiter: &RateLimiter) -> ResponseResult<bool> {
    if rate_limiter.is_rate_limited(msg.chat.id).await {
        if let Some(remaining_time) = rate_limiter.get_remaining_time(msg.chat.id).await {
            let remaining_seconds = remaining_time.as_secs();
            bot.send_message(msg.chat.id, format!("Я Дора, чай закончился и я не смогу скачать тебе трек сейчас. Попробуй попозже через {} секунд.", remaining_seconds)).await?;
        } else {
            bot.send_message(msg.chat.id, "Я Дора, чай закончился и я не смогу скачать тебе трек сейчас. Попробуй попозже.").await?;
        }
        return Ok(false);
    }
    rate_limiter.update_rate_limit(msg.chat.id).await;
    Ok(true)
}

pub async fn download_and_send_audio(bot: Bot, msg: Message, url: Url, rate_limiter: Arc<RateLimiter>) -> ResponseResult<()> {
    let bot_clone = bot.clone();
    let chat_id = msg.chat.id;
    let rate_limiter = Arc::clone(&rate_limiter);

    // Create a custom reqwest::Client with a longer timeout
    // let client = Client::builder()
    //     .timeout(Duration::from_secs(60)) // Set timeout to 60 seconds
    //     .build()
    //     .expect("Failed to create reqwest client");

    tokio::spawn(async move {
        let result: Result<(), CommandError> = async {
            // let client = Client::new();
            // let url_str = format!("https://api.telegram.org/token:redacted/GetUpdates");
            // let updates = get_updates_with_retry(&client, &url_str).await.map_err(|e| {
                // CommandError::Network(anyhow!("Failed to get updates: {}", e))
            // })?;

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
                .expect("Failed to start youtube-dl process");
            let _ = child.wait().expect("youtube-dl process failed");

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
                .expect("Failed to execute ffprobe");
            let duration_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let duration: u32 = duration_str.parse::<f32>().unwrap_or(0.0).round() as u32;

            bot_clone
                .send_audio(chat_id, InputFile::file(&download_path))
                .duration(duration)
                .await
                .map_err(|e| CommandError::Download(anyhow!("Failed to send audio file: {}", e)))?;
                
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

    bot.send_message(msg.chat.id, "Я Дора, попробую скачать тебе трек! ❤️‍🔥 Терпение!").await?;
    Ok(())
}

pub async fn download_and_send_video(bot: Bot, msg: Message, url: Url, rate_limiter: Arc<RateLimiter>) -> ResponseResult<()> {
    let bot_clone = bot.clone();
    let chat_id = msg.chat.id;
    let rate_limiter = Arc::clone(&rate_limiter);

    tokio::spawn(async move {
        let result: Result<(), CommandError> = async {
            let file_name = "video.mp4"; // You can generate a better name based on metadata if needed
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

            bot_clone
                .send_video(chat_id, InputFile::file(&download_path))
                .await
                .map_err(|e| CommandError::Download(anyhow!("Failed to send video file: {}", e)))?;
                
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

    bot.send_message(msg.chat.id, "Я Дора, попробую скачать тебе видео! 🎥 Терпение!").await?;
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

pub async fn handle_message(bot: Bot, msg: Message, rate_limiter: Arc<RateLimiter>) -> ResponseResult<()> {
    if let Some(text) = msg.text() {
        println!("handle_message {:?}", msg.text());
        if text.starts_with("/start") || text.starts_with("/help") {
            return Ok(());
        }
        let is_video = text.starts_with("video ");
        let url_text = if is_video { &text[6..] } else { text };
        let mut url = match Url::parse(url_text) {
            Ok(parsed_url) => parsed_url,
            Err(_) => {
                bot.send_message(msg.chat.id, "Извини, я не смогла распознать ссылку. Пожалуйста, пришли мне корректную ссылку на YouTube или SoundCloud.").await?;
                return Ok(());
            }
        };

        // Remove the &list parameter if it exists
        if url.query_pairs().any(|(key, _)| key == "list") {
            let preserved_params: Vec<(String, String)> = url.query_pairs()
                .filter(|(key, _)| key != "list")
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect();
            
            url.query_pairs_mut().clear();
            
            for (key, value) in preserved_params {
                url.query_pairs_mut().append_pair(&key, &value);
            }
        }

        if handle_rate_limit(&bot, &msg, &rate_limiter).await? {
            if is_video {
                download_and_send_video(bot, msg, url, rate_limiter).await?;
            } else {
                download_and_send_audio(bot, msg, url, rate_limiter).await?;
            }
        }
    } else {
        bot.send_message(msg.chat.id, "Извини, я не нашла ссылки на YouTube или SoundCloud. Пожалуйста, пришли мне ссылку на трек или видео, который ты хочешь скачать.").await?;
    }
    Ok(())
}