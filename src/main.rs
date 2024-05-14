mod commands;
mod errors;
mod fetch;
mod rate_limiter;

use commands::handle_message;
use rate_limiter::RateLimiter;
use teloxide::prelude::*;
use tokio::time::Duration;

const RATE_LIMIT_DURATION: Duration = Duration::from_secs(30); // Set rate limit duration

#[tokio::main]
async fn main() {
    pretty_env_logger::init();
    log::info!("Starting throw dice bot...");

    let bot = Bot::from_env().auto_send();
    let rate_limiter = RateLimiter::new(RATE_LIMIT_DURATION);
    let rate_limiter = rate_limiter.clone();

    teloxide::repl(bot, move |bot: AutoSend<Bot>, msg: Message| {
        let rate_limiter = rate_limiter.clone();
        async move {
            handle_message(bot, msg, &rate_limiter).await
        }
    })
    .await;
}
