use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::ParseMode;
use teloxide::utils::command::BotCommands;
use std::time::Duration;
use anyhow::Result;
use tokio::signal;

mod commands;
mod db;
mod fetch;
mod rate_limiter;
mod utils;

use db::get_connection;
use crate::commands::handle_message;
use crate::rate_limiter::RateLimiter;

#[derive(BotCommands, Clone)]
enum Command {
    #[command(rename = "help", description = "Расскажу что я могу, помимо вкусного чая")]
    Help,
    #[command(rename = "start", description = "Познакомимся")]
    Start,
}

#[tokio::main]
async fn main() -> Result<()> {
    let bot = Bot::from_env();
    let conn = get_connection()?;  // Ensure this line uses the `?` operator correctly

    let rate_limiter = Arc::new(RateLimiter::new(Duration::from_secs(30)));

    let bot_repl = Command::repl(bot, move |bot: Bot, msg: Message, cmd: Command| {
            let rate_limiter = Arc::clone(&rate_limiter); // Clone the Arc for each closure invocation
            async move {
                match cmd {
                    Command::Start => {
                        println!("Start {:?}", msg);
                        bot.send_message(msg.chat.id, "Приветик! Я Дора ❤️‍🔥. Я делаю чай и скачиваю треки. Используй /help чтобы увидить больше.")
                            .parse_mode(ParseMode::MarkdownV2)
                            .await?;
                    }
                    Command::Help => {
                        bot.send_message(msg.chat.id, Command::descriptions().to_string())
                            .parse_mode(ParseMode::MarkdownV2)
                            .await?;
                    }
                }
                if let Err(err) = handle_message(bot, msg, rate_limiter).await {
                    log::error!("Error handling message: {:?}", err);
                }
                respond(())
            }
        }
    );

    tokio::select! {
        _ = bot_repl => {},
        _ = signal::ctrl_c() => {
            println!("Shutting down gracefully...");
        },
    }

    Ok(())
}
