use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{KeyboardButton, KeyboardMarkup, ParseMode, Message, BotCommand};
use teloxide::utils::command::BotCommands;
use teloxide::dispatching::{UpdateFilterExt, Dispatcher};
use url::Url;
use std::time::Duration;
use anyhow::Result;
use tokio::signal;

mod commands;
mod db;
mod fetch;
mod rate_limiter;
mod utils;

use db::get_connection;
use crate::commands::{handle_message, download_and_send_audio, handle_rate_limit};
use crate::rate_limiter::RateLimiter;

#[derive(BotCommands, Clone)]
#[command(description = "Мои команды:")]
enum Command {
    #[command(description = "показывает это сообщение")]
    Help,
    #[command(description = "показывает главное меню")]
    Start,
    #[command(description = "показывает настройки")]
    Settings,
}

#[tokio::main]
async fn main() -> Result<()> {
    pretty_env_logger::init();
    log::info!("Starting bot...");

    let bot = Bot::from_env();
    
    // Set the list of bot commands
    bot.set_my_commands(vec![
        BotCommand::new("start", "показывает главное меню"),
        BotCommand::new("help", "расскажу что я могу, помимо вкусного чая"),
        BotCommand::new("settings", "твои настройки"),
    ])
    .await?;

    let conn = get_connection()?; // Ensure this line uses the `?` operator correctly
    let rate_limiter = Arc::new(RateLimiter::new(Duration::from_secs(30)));

    // Create a dispatcher to handle both commands and plain messages
    let handler = dptree::entry()
        .branch(Update::filter_message().branch(
            dptree::entry()
                .filter_command::<Command>()
                .endpoint(|bot: Bot, msg: Message, cmd: Command| async move {
                    match cmd {
                        Command::Start => {
                            // let keyboard = make_menu();
                            bot.send_message(msg.chat.id, "Приветик! Я Дора ❤️‍🔥. Я делаю чай и скачиваю треки. Используй /help чтобы получить полную инфу.")
                                .parse_mode(ParseMode::MarkdownV2)
                                // .reply_markup(keyboard)
                                .await?;
                        }
                        Command::Help => {
                            // let keyboard = make_menu();
                            bot.send_message(msg.chat.id, Command::descriptions().to_string())
                                .parse_mode(ParseMode::MarkdownV2)
                                // .reply_markup(keyboard)
                                .await?;
                        }
                        Command::Settings => {
                            bot.send_message(msg.chat.id, "Ты можешь качать трек, каждые 30 секунд!")
                                .parse_mode(ParseMode::MarkdownV2)
                                // .reply_markup(keyboard)
                                .await?;
                        }                        
                    }
                    respond(())
                })
        ))
        .branch(Update::filter_message().endpoint({
            let rate_limiter = Arc::clone(&rate_limiter);
            move |bot: Bot, msg: Message| {
                let rate_limiter = Arc::clone(&rate_limiter);
                async move {
                    if let Err(err) = handle_message(bot, msg, rate_limiter).await {
                        log::error!("Error handling message: {:?}", err);
                    }
                    respond(())
                }
            }
        }));

        let mut dispatcher = Dispatcher::builder(bot.clone(), handler)
        .default_handler(|_| async {})
        .build();

        // Start the dispatcher
        dispatcher.dispatch().await;

    tokio::select! {
        _ = signal::ctrl_c() => {
            println!("Shutting down gracefully...");
        },
    }

    Ok(())
}

fn make_menu() -> KeyboardMarkup {
    let buttons = vec![
        vec![
            KeyboardButton::new("Option 1"),
            KeyboardButton::new("Option 2"),
        ],
        vec![
            KeyboardButton::new("Option 3"),
            KeyboardButton::new("Option 4"),
        ],
    ];
    KeyboardMarkup::new(buttons)
        .resize_keyboard(true)
        .one_time_keyboard(false)
}
