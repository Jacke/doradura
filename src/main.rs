use std::sync::Arc;
use teloxide::prelude::*;
use std::time::Duration;
use anyhow::Result;

mod commands;
mod db;
mod fetch;
mod rate_limiter;
mod utils;

use db::get_connection;
use crate::commands::handle_message;
use crate::rate_limiter::RateLimiter;

#[tokio::main]
async fn main() -> Result<()> {
    let bot = Bot::from_env();
    let conn = get_connection()?;  // Ensure this line uses the `?` operator correctly

    let rate_limiter = Arc::new(RateLimiter::new(Duration::from_secs(30)));

    teloxide::repl(bot, move |bot: Bot, msg: Message| {
        let rate_limiter = Arc::clone(&rate_limiter);
        async move {
            if let Err(err) = handle_message(bot, msg, rate_limiter).await {
                log::error!("Error handling message: {:?}", err);
            }
            respond(())
        }
    })
    .await;

    Ok(())
}
