use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use axum::body::{to_bytes, Body};
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use teloxide::dispatching::{Dispatcher, UpdateHandler};
use teloxide::prelude::*;
use teloxide::requests::{HasPayload, Request as TelegramRequest};
use teloxide::types::UserId;
use teloxide::update_listeners::webhooks::{self, Options};
use teloxide::update_listeners::UpdateListener;

use crate::core::config;
use crate::storage::SharedStorage;
use crate::telegram::handlers::HandlerError;
use crate::telegram::Bot;

const WEBHOOK_BODY_LIMIT_BYTES: usize = 1024 * 1024;
const TELEGRAM_SECRET_HEADER: &str = "X-Telegram-Bot-Api-Secret-Token";

#[derive(Clone)]
struct DedupState {
    shared_storage: Arc<SharedStorage>,
    bot_id: i64,
    secret_token: Arc<str>,
}

pub async fn run_webhook_mode(
    bot: Bot,
    handler: UpdateHandler<HandlerError>,
    shared_storage: Arc<SharedStorage>,
    bot_id: UserId,
    bot_init_start: std::time::Instant,
) -> Result<()> {
    let public_url = config::WEBHOOK_URL
        .clone()
        .ok_or_else(|| anyhow!("WEBHOOK_URL must be set when running in webhook mode"))?;
    let listen_addr = parse_listen_addr()?;
    let secret_token = config::WEBHOOK_SECRET_TOKEN
        .clone()
        .ok_or_else(|| anyhow!("WEBHOOK_SECRET_TOKEN must be set when running in webhook mode"))?;
    let public_url = url::Url::parse(&public_url).context("parse WEBHOOK_URL")?;
    let path = config::WEBHOOK_PATH.clone();

    let options = Options::new(listen_addr, public_url.clone())
        .path(path.clone())
        .secret_token(secret_token.clone());

    let (mut listener, stop_flag, router) = webhooks::axum_no_setup(options);
    let stop_token = listener.stop_token();

    let app = router.layer(middleware::from_fn_with_state(
        DedupState {
            shared_storage,
            bot_id: bot_id.0 as i64,
            secret_token: Arc::<str>::from(secret_token),
        },
        dedup_middleware,
    ));

    let server = tokio::spawn(async move {
        let tcp_listener = tokio::net::TcpListener::bind(listen_addr)
            .await
            .with_context(|| format!("bind webhook listener at {}", listen_addr))?;
        axum::serve(tcp_listener, app)
            .with_graceful_shutdown(stop_flag)
            .await
            .context("run webhook axum server")?;
        Ok::<(), anyhow::Error>(())
    });

    let init_elapsed = bot_init_start.elapsed();
    log::info!("Starting bot in webhook mode");
    log::info!("Webhook public URL: {}", public_url);
    log::info!("Webhook listen address: {}", listen_addr);
    log::info!("Webhook path: {}", path);
    log::info!("================================================");
    log::info!("🎉 Bot initialization complete in {:.2}s", init_elapsed.as_secs_f64());
    log::info!("📡 Ready to receive webhook updates");
    log::info!("================================================");

    let mut dispatcher = Dispatcher::builder(bot, handler)
        .dependencies(DependencyMap::new())
        .build();

    tokio::select! {
        _ = dispatcher.dispatch_with_listener(
            listener,
            LoggingErrorHandler::with_custom_text("An error from the webhook listener"),
        ) => {
            stop_token.stop();
        }
        _ = tokio::signal::ctrl_c() => {
            log::info!("Received shutdown signal, stopping webhook listener");
            stop_token.stop();
        }
    }

    server.await.context("join webhook server task")??;
    Ok(())
}

async fn dedup_middleware(State(state): State<DedupState>, request: Request<Body>, next: Next) -> Response {
    let header_secret = request
        .headers()
        .get(TELEGRAM_SECRET_HEADER)
        .and_then(|value| value.to_str().ok());
    if header_secret != Some(state.secret_token.as_ref()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let (parts, body) = request.into_parts();
    let bytes = match to_bytes(body, WEBHOOK_BODY_LIMIT_BYTES).await {
        Ok(bytes) => bytes,
        Err(err) => {
            log::warn!("Failed to read webhook body: {}", err);
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let update_id = match serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|value| value.get("update_id").and_then(|value| value.as_i64()))
    {
        Some(update_id) => update_id,
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    match state
        .shared_storage
        .register_processed_update(state.bot_id, update_id)
        .await
    {
        Ok(true) => {
            let request = Request::from_parts(parts, Body::from(bytes));
            next.run(request).await
        }
        Ok(false) => StatusCode::OK.into_response(),
        Err(err) => {
            log::warn!("Failed to register processed update {}: {}", update_id, err);
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}

pub async fn set_webhook(bot: &Bot, drop_pending_updates: bool) -> Result<()> {
    let public_url = config::WEBHOOK_URL
        .clone()
        .ok_or_else(|| anyhow!("WEBHOOK_URL must be set"))?;
    let secret_token = config::WEBHOOK_SECRET_TOKEN
        .clone()
        .ok_or_else(|| anyhow!("WEBHOOK_SECRET_TOKEN must be set"))?;
    let mut request = bot.set_webhook(url::Url::parse(&public_url).context("parse WEBHOOK_URL")?);
    request.payload_mut().secret_token = Some(secret_token);
    request.payload_mut().drop_pending_updates = Some(drop_pending_updates);
    request.payload_mut().max_connections = *config::WEBHOOK_MAX_CONNECTIONS;
    request.send().await.context("set Telegram webhook")?;
    Ok(())
}

pub async fn delete_webhook(bot: &Bot, drop_pending_updates: bool) -> Result<()> {
    let mut request = bot.delete_webhook();
    request.payload_mut().drop_pending_updates = Some(drop_pending_updates);
    request.send().await.context("delete Telegram webhook")?;
    Ok(())
}

pub async fn print_webhook_info(bot: &Bot) -> Result<()> {
    let info = bot.get_webhook_info().send().await.context("get webhook info")?;
    println!("url: {:?}", info.url);
    println!("pending_update_count: {}", info.pending_update_count);
    println!("max_connections: {:?}", info.max_connections);
    println!("ip_address: {:?}", info.ip_address);
    println!("has_custom_certificate: {}", info.has_custom_certificate);
    println!("allowed_updates: {:?}", info.allowed_updates);
    println!("last_error_date: {:?}", info.last_error_date);
    println!("last_error_message: {:?}", info.last_error_message);
    Ok(())
}

fn parse_listen_addr() -> Result<SocketAddr> {
    config::WEBHOOK_LISTEN_ADDR
        .parse()
        .with_context(|| format!("parse WEBHOOK_LISTEN_ADDR ({})", *config::WEBHOOK_LISTEN_ADDR))
}
