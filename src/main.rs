// src/main.rs
use std::sync::Arc;
use teloxide::prelude::*;
use std::time::Duration;

mod commands;
mod fetch;
mod rate_limiter;
mod utils;


use crate::commands::handle_message;
use crate::rate_limiter::RateLimiter;

#[tokio::main]
async fn main() {
    let bot = Bot::from_env();

    let rate_limiter = Arc::new(RateLimiter::new(Duration::from_secs(30)));

    teloxide::repl(bot, move |bot: Bot, msg: Message| {
        let rate_limiter = Arc::clone(&rate_limiter);
        async move {
            handle_message(bot, msg, rate_limiter).await?;
            respond(())
        }
    })
    .await;
}
