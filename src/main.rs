use std::fs::read_to_string;
use std::sync::Arc;
use std::hash::Hash;
use teloxide::prelude::*;
use teloxide::types::{KeyboardButton, KeyboardMarkup, ParseMode, Message, BotCommand};
use teloxide::utils::command::BotCommands;
use teloxide::dispatching::{UpdateFilterExt, Dispatcher, DefaultKey};
use std::time::Duration;
use anyhow::Result;
use tokio::signal;
use dptree::di::DependencyMap;
use reqwest::ClientBuilder;
use tokio::time::sleep;
use log::{error, warn, info, debug, trace};
use pretty_env_logger;
use simplelog::*;
use std::fs::File;
use reqwest::Client;
use reqwest::Error as ReqwestError;
use tokio_retry::strategy::ExponentialBackoff;
use tokio_retry::Retry;

mod commands;
mod db;
mod fetch;
mod rate_limiter;
mod utils;

use db::{get_connection, create_user, get_user, log_request};
use crate::commands::handle_message;
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
    // Initialize simplelog for both console and file logging
    CombinedLogger::init(vec![
        TermLogger::new(
            LevelFilter::Error,
            Config::default(),
            TerminalMode::Mixed,
            ColorChoice::Auto,
        ),
        WriteLogger::new(
            LevelFilter::Error,
            Config::default(),
            File::create("app.log").unwrap(),
        ),
    ])
    .unwrap();
    
    log::info!("Starting bot...");

    let bot = Bot::from_env_with_client(
        ClientBuilder::new()
            .timeout(Duration::from_secs(30)) // Increase request timeout to 30 seconds
            .build()?,
    );

    let mut retry_count = 0;
    let max_retries = 5;

    // Set the list of bot commands
    bot.set_my_commands(vec![
        BotCommand::new("start", "показывает главное меню"),
        BotCommand::new("help", "расскажу что я могу, помимо вкусного чая"),
        BotCommand::new("settings", "твои настройки"),
    ])
    .await?;

    // Read and apply the migration.sql file
    let migration_sql = read_to_string("migration.sql")?;
    let conn = get_connection()?;
    conn.execute_batch(&migration_sql)?;

    let rate_limiter = Arc::new(RateLimiter::new(Duration::from_secs(30)));

    // Create a dispatcher to handle both commands and plain messages
    let handler = dptree::entry()
        .branch(Update::filter_message().branch(
            dptree::entry()
                .filter_command::<Command>()
                .endpoint(|bot: Bot, msg: Message, cmd: Command| async move {
                    match cmd {
                        Command::Start => {
                            bot.send_message(msg.chat.id, "Приветик! Я Дора ❤️‍🔥. Я делаю чай и скачиваю треки. Используй /help чтобы получить полную инфу.")
                                .parse_mode(ParseMode::MarkdownV2)
                                .await?;
                        }
                        Command::Help => {
                            bot.send_message(msg.chat.id, Command::descriptions().to_string())
                                .parse_mode(ParseMode::MarkdownV2)
                                .await?;
                        }
                        Command::Settings => {
                            bot.send_message(msg.chat.id, "Ты можешь качать трек, каждые 30 секунд!")
                                .parse_mode(ParseMode::MarkdownV2)
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
                    if let Err(err) = handle_message(bot, msg.clone(), rate_limiter).await {
                        log::error!("Error handling message: {:?}", err);
                    }

                    // Log request and manage user
                    let conn = get_connection().unwrap();
                    let chat_id = msg.chat.id.0; // Extract i64 from ChatId
                    if let Some(user) = get_user(&conn, chat_id).unwrap() {
                        log_request(&conn, user.telegram_id(), &msg.text().unwrap()).unwrap();
                    } else {
                        create_user(&conn, chat_id, msg.from().and_then(|u| u.username.clone())).unwrap();
                        log_request(&conn, chat_id, &msg.text().unwrap()).unwrap();
                    }

                    respond(())
                }
            }
        }));

    // Run the dispatcher with retry logic
    loop {
        let mut dispatcher = Dispatcher::builder(bot.clone(), handler.clone())
            .dependencies(DependencyMap::new())
            .default_handler(|_| async {})
            .build();

        if let Err(err) = run_dispatcher(&mut dispatcher).await {
            log::error!("Dispatcher error: {:?}", err);
            if retry_count < max_retries {
                retry_count += 1;
                exponential_backoff(retry_count).await;
            } else {
                log::error!("Max retries reached. Exiting...");
                break;
            }
        } else {
            retry_count = 0; // Reset retry count on success
        }

        // Add a delay between retries to avoid overwhelming the API
        if retry_count > 0 {
            sleep(Duration::from_secs(5)).await;
        }
    }

    tokio::select! {
        _ = signal::ctrl_c() => {
            println!("Shutting down gracefully...");
        },
    }

    Ok(())
}

async fn run_dispatcher<R, E, K>(dispatcher: &mut Dispatcher<R, E, K>) -> Result<(), ()>
where
    R: Requester + Clone + Send + Sync + 'static,
    R::GetUpdates: Send,
    R::SendMessage: Send,
    E: std::error::Error + Send + Sync + 'static,
    K: Eq + Hash + Clone + Send + Sync + 'static,
{
    dispatcher.dispatch().await;
    Ok(())
}

async fn exponential_backoff(retry_count: u32) {
    let delay = Duration::from_secs(2u64.pow(retry_count));
    tokio::time::sleep(delay).await;
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

async fn get_updates_with_retry(client: &Client, url: &str) -> Result<String, ReqwestError> {
    let retry_strategy = ExponentialBackoff::from_millis(100).take(5);

    let response = Retry::spawn(retry_strategy, || async {
        client
            .get(url)
            .timeout(Duration::from_secs(10)) // Set a timeout for the request
            .send()
            .await?
            .text()
            .await
    })
    .await?;

    Ok(response)
}