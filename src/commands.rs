use teloxide::prelude::*;
use teloxide::types::InputFile;
use url::Url;
use uuid::Uuid;
use std::fs;
use std::path::PathBuf;
use shellexpand;
use rand::{thread_rng, Rng};
use rand::seq::SliceRandom;
use tokio::process::Command;
use tokio::time::{sleep, Duration};
use crate::fetch::fetch_song_metadata;
use crate::rate_limiter::RateLimiter;

pub async fn handle_message(bot: AutoSend<Bot>, msg: Message, rate_limiter: &RateLimiter) -> Result<(), teloxide::RequestError> {
    if let Some(text) = msg.text() {
        if text.contains("youtube.com") || text.contains("youtu.be") || text.contains("soundcloud.com") {
            let cleaned_url = Url::parse(text).unwrap_or_else(|_| Url::parse("").unwrap());

            let verses = [
                "Закрываю дверь квартиры\nОтключаю все мобилы\nНедоступна для дебилов\nПотому что я влюбилась\nВ тебя-а-а, тупого наглеца\nОт чего же? От чего же?",
                "Я увидела твой взгляд\nЗаострённый на мне\nТы рукою помахал\nЯ помахала в ответ\nТы пошёл ко мне навстречу\nЭто было так глупо\nВедь за спиною моей\nСтояла твоя подруга (Подруга)",
                "Ты позвал меня на встречу (А)\nТы позвал меня на встречу\nЯ готовилась весь вечер\nВыбирала, что надеть мне\nИстрепала свои нервы\nПришла, ждала почти два часа\nИ ты написал: «Сорри, я проспал»"
            ];

            let selected_verse = verses.choose(&mut thread_rng()).unwrap_or(&verses[0]);
            bot.send_message(msg.chat.id, "Я Дора, попробую скачать тебе трек! ❤️‍🔥 Терпение!".to_string()).await?;
            bot.send_message(msg.chat.id, selected_verse.to_string()).await?;

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

            let (title, artist) = fetch_song_metadata(&cleaned_url.as_str()).await.unwrap_or(("Unknown".to_string(), "Unknown".to_string()));
            let file_name = if artist.trim().is_empty() && title.trim().is_empty() {
                "Unknown.%(ext)s".to_string()
            } else if artist.trim().is_empty() {
                format!("{}.%(ext)s", title)
            } else if title.trim().is_empty() {
                format!("{}.%(ext)s", artist)
            } else {
                format!("{} - {}.%(ext)s", artist, title)
            };

            fn escape_filename(filename: &str) -> String {
                shell_escape::unix::escape(filename.chars().collect()).to_string()
            }

            let safe_filename = escape_filename(&file_name);
            let full_path = format!("/Users/stasobolev/downloads/{}", safe_filename);
            let download_path = shellexpand::tilde(&full_path).into_owned();

            let download_cmd = format!("youtube-dl -o {} --extract-audio --audio-format mp3 --add-metadata --embed-thumbnail '{}'", download_path, cleaned_url);
        
            let mut child = Command::new("sh")
                .arg("-c")
                .arg(&download_cmd)
                .spawn()
                .expect("Failed to start youtube-dl process");
            let _ = child.wait().await.expect("youtube-dl process failed");
            println!("download_path {:?}", download_path);
            let final_path = download_path.replace("'", "").replace("%(ext)s", "mp3");
            println!("final_path {:?}", download_path);
            bot.send_audio(msg.chat.id, InputFile::file(final_path)).await?;

            tokio::spawn(async move {
                sleep(Duration::from_secs(600)).await;
                println!("full_path {:?}", &full_path);
                fs::remove_file(&full_path).expect("Failed to delete file");
            });
        }
    }

    Ok(())
}
