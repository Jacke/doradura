use teloxide::prelude::*;
use teloxide::types::InputFile;
use crate::rate_limiter::RateLimiter;
use crate::fetch::fetch_song_metadata;
use crate::utils::{escape_filename, download_file};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use url::Url;
use thiserror::Error;
use anyhow::{Error, anyhow};

#[derive(Error, Debug)]
enum CommandError {
    #[error("Failed to fetch song metadata")]
    FetchMetadata(#[from] Error),
    #[error("Failed to download file")]
    Download(Error),
}

pub async fn handle_message(bot: Bot, msg: Message, rate_limiter: Arc<RateLimiter>) -> ResponseResult<()> {
    if let Some(text) = msg.text() {
        if text.contains("youtube.com") || text.contains("youtu.be") || text.contains("soundcloud.com") {
            if rate_limiter.is_rate_limited(msg.chat.id).await {
                if let Some(remaining_time) = rate_limiter.get_remaining_time(msg.chat.id).await {
                    let remaining_seconds = remaining_time.as_secs();
                    bot.send_message(msg.chat.id, format!("Я Дора, чай закончился и я не смогу скачать тебе трек сейчас. Попробуй попозже через {} секунд.", remaining_seconds)).await?;
                } else {
                    bot.send_message(msg.chat.id, "Я Дора, чай закончился и я не смогу скачать тебе трек сейчас. Попробуй попозже.").await?;
                }
                return Ok(());
            }
            rate_limiter.update_rate_limit(msg.chat.id).await;
            let url = Url::parse(text).unwrap_or_else(|_| Url::parse("").unwrap());
            let bot_clone = bot.clone();
            let chat_id = msg.chat.id;
            let rate_limiter = Arc::clone(&rate_limiter);
            tokio::spawn(async move {
                let result: Result<(), CommandError> = async {
                    let (title, artist) = fetch_song_metadata(&url.as_str())
                        .await
                        .map_err(|e| CommandError::FetchMetadata(anyhow!("Failed to fetch song metadata: {}", e)))?;
                    let file_name = if artist.trim().is_empty() && title.trim().is_empty() {
                        "Unknown.mp3".to_string()
                    } else if artist.trim().is_empty() {
                        format!("{}.mp3", title)
                    } else if title.trim().is_empty() {
                        format!("{}.mp3", artist)
                    } else {
                        format!("{} - {}.mp3", artist, title)
                    };
                    let safe_filename = escape_filename(&file_name);
                    let full_path = format!("~/downloads/{}", safe_filename);
                    let download_path = shellexpand::tilde(&full_path).into_owned();
                    let cleaned_url = url.as_str().replace("'", "'\\''");
                    let download_cmd = format!("youtube-dl -o {} --extract-audio --audio-format mp3 --add-metadata --embed-thumbnail '{}'", download_path, cleaned_url);
                    let mut child = Command::new("youtube-dl")
                      .arg("-o")
                      .arg(&download_path)
                      .arg("--extract-audio")
                      .arg("--audio-format")
                      .arg("mp3")
                      .arg("--add-metadata")
                      .arg("--embed-thumbnail")
                      .arg(url.as_str())
                      .spawn()
                      .expect("Failed to start youtube-dl process");
                
                    let _ = child.wait().expect("youtube-dl process failed");
                    println!("download_path {:?}", download_path);
                    bot_clone
                        .send_audio(chat_id, InputFile::file(&download_path))
                        .await
                        .map_err(|e| CommandError::Download(anyhow!("Failed to send audio file: {}", e)))?;
                    tokio::time::sleep(Duration::from_secs(600)).await;
                    std::fs::remove_file(&download_path).expect("Failed to delete file");
                    Ok(())
                }.await;
                if let Err(e) = result {
                    bot_clone.send_message(chat_id, format!("An error occurred: {}", e))
                        .await
                        .unwrap();
                }
            });
            bot.send_message(msg.chat.id, "Я Дора, попробую скачать тебе трек! ❤️‍🔥 Терпение!").await?;
        } else {
          bot.send_message(msg.chat.id, "Извини, я не нашла ссылки на YouTube или SoundCloud. Пожалуйста, пришли мне ссылку на трек, который ты хочешь скачать. Потом я налью тебе чай и скачаю ❤️‍🔥").await?;
        }
    }
    Ok(())
}