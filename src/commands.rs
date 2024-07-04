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
use crate::queue::{DownloadTask, DownloadQueue};

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

pub async fn handle_message(bot: Bot, msg: Message, download_queue: Arc<DownloadQueue>, rate_limiter: Arc<RateLimiter>) -> ResponseResult<()> {
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
                println!("handle_rate_limit is_video add_task");
                let task = DownloadTask { url: url.to_string(), chat_id: msg.chat.id, is_video: true };
                download_queue.add_task(task);
                bot.send_message(msg.chat.id, "Я Дора, попробую скачать тебе видео! 🎥 Терпение!").await?;
            } else {
                let task = DownloadTask { url: url.to_string(), chat_id: msg.chat.id, is_video: false };
                println!("handle_rate_limit not video add_task");
                download_queue.add_task(task);
                bot.send_message(msg.chat.id, "Я Дора, попробую скачать тебе трек! ❤️‍🔥 Терпение!").await?;
            }            
        }
    } else {
        bot.send_message(msg.chat.id, "Извини, я не нашла ссылки на YouTube или SoundCloud. Пожалуйста, пришли мне ссылку на трек или видео, который ты хочешь скачать.").await?;
    }
    Ok(())
}