use teloxide::prelude::*;
use teloxide::types::InputFile;
use tokio::process::Command;
use tokio::time::{sleep, Duration};
use url::Url;
use uuid::Uuid;
use std::fs;
use std::path::PathBuf;
use shellexpand;
use rand::{thread_rng, Rng};
use rand::seq::SliceRandom;
use reqwest;
use select::document::Document;
use select::predicate::Name;

async fn fetch_song_metadata(url: &str) -> Result<(String, String), reqwest::Error> {
    let resp = reqwest::get(url).await?.text().await?;
    let document = Document::from(resp.as_str());

    let title = document.find(Name("title")).next().map(|n| n.text()).unwrap_or_default();

    let artist = document.find(Name("meta"))
        .filter(|n| n.attr("property").map(|v| v == "og:artist").unwrap_or(false))
        .next()
        .and_then(|n| n.attr("content"))
        .unwrap_or_default()
        .to_string();

    Ok((title, artist))
}


#[tokio::main]
async fn main() {
    pretty_env_logger::init();
    log::info!("Starting throw dice bot...");

    let bot = Bot::from_env().auto_send();

    teloxide::repl(bot, |bot: AutoSend<Bot>, msg: Message| async move {
        if let Some(text) = msg.text() {
            if text.contains("youtube.com") || text.contains("youtu.be") || text.contains("soundcloud.com") {
                let cleaned_url = Url::parse(text).unwrap_or_else(|_| Url::parse("").unwrap());
                // let mut query_pairs = url.query_pairs_mut();
                //  query_pairs.remove("list");
                // let cleaned_url = url.to_string();

                // Lyrics array
                let verses = [
                    "Закрываю дверь квартиры\nОтключаю все мобилы\nНедоступна для дебилов\nПотому что я влюбилась\nВ тебя-а-а, тупого наглеца\nОт чего же? От чего же?",
                    "Я увидела твой взгляд\nЗаострённый на мне\nТы рукою помахал\nЯ помахала в ответ\nТы пошёл ко мне навстречу\nЭто было так глупо\nВедь за спиною моей\nСтояла твоя подруга (Подруга)",
                    "Ты позвал меня на встречу (А)\nТы позвал меня на встречу\nЯ готовилась весь вечер\nВыбирала, что надеть мне\nИстрепала свои нервы\nПришла, ждала почти два часа\nИ ты написал: «Сорри, я проспал»"
                ];

                // Select a random verse
                let selected_verse = verses.choose(&mut thread_rng()).unwrap_or(&verses[0]);
                // Send a random verse from the song
                bot.send_message(msg.chat.id, "Я Дора, попробую скачать тебе трек! ❤️‍🔥 Терпение!".to_string()).await?;
                bot.send_message(msg.chat.id, selected_verse.to_string()).await?;



                // Handle media download
                // let output = format!("{}.mp4", Uuid::new_v4()); // Assume MP4 by default
                let (title, artist) = fetch_song_metadata(&cleaned_url.as_str()).await.unwrap_or(("Unknown".to_string(), "Unknown".to_string()));
                let file_name = if artist.trim().is_empty() && title.trim().is_empty() {
                    "Unknown.%(ext)s".to_string()  // Convert to String
                } else if artist.trim().is_empty() {
                    format!("{}.%(ext)s", title)
                } else if title.trim().is_empty() {
                    format!("{}.%(ext)s", artist)
                } else {
                    format!("{} - {}.%(ext)s", artist, title)
                };

                fn escape_filename(filename: &str) -> String {
                    // Use shell-escape to ensure that filenames are safely quoted for use in shell commands
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

                /*
                // Send the file based on its type
                match full_path.extension().and_then(|ext| ext.to_str()) {
                    Some("mp4") => {
                        bot.send_video(msg.chat.id, InputFile::file(full_path.clone())).await?;
                    },
                    Some("mp3") => {
                        bot.send_audio(msg.chat.id, InputFile::file(full_path.clone())).await?;
                    },
                    _ => {
                        log::warn!("Unsupported file type or file extension not recognized.");
                    }
                }
*/                

                // Schedule file deletion after 10 minutes
                tokio::spawn(async move {
                    sleep(Duration::from_secs(600)).await;  // Wait for 10 minutes
                    println!("full_path {:?}", &full_path);
                    fs::remove_file(&full_path).expect("Failed to delete file");
                });
            }
        }
        // Your existing logic for sending photo and dice
        // let photo_url = Url::parse("https://pi.math.cornell.edu/~mec/2006-2007/Probability/Yahtzee5.jpg").expect("Invalid URL");
        // bot.send_photo(msg.chat.id, InputFile::url(photo_url)).await?;
        // bot.send_dice(msg.chat.id).await?;

        Ok(())
    })
    .await;
}
