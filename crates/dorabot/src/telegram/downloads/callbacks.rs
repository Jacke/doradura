use std::sync::Arc;

use teloxide::prelude::*;
use teloxide::types::{CallbackQueryId, MessageId};

use crate::downsub::DownsubGateway;
use crate::storage::{DbPool, SharedStorage, SubtitleCache};
use crate::telegram::Bot;

use super::CallbackCtx;

/// Handle downloads callback queries
pub async fn handle_downloads_callback(
    bot: &Bot,
    callback_id: CallbackQueryId,
    chat_id: ChatId,
    message_id: MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
    username: Option<String>,
    downsub_gateway: Arc<DownsubGateway>,
    subtitle_cache: Arc<SubtitleCache>,
) -> ResponseResult<()> {
    log::info!("handle_downloads_callback called with data: {}", data);
    bot.answer_callback_query(callback_id).await?;

    let parts: Vec<&str> = data.splitn(6, ':').collect();
    log::info!("Parsed parts: {:?}", parts);
    if parts.len() < 2 {
        log::warn!("Not enough parts in callback data");
        return Ok(());
    }

    let action = parts[1];
    log::info!("Action: {}", action);

    let ctx = CallbackCtx {
        bot: bot.clone(),
        chat_id,
        message_id,
        db_pool,
        shared_storage,
        username,
        downsub_gateway,
        subtitle_cache,
    };

    match action {
        // Send / navigation
        "page" | "filter" | "catfilter" | "resend" | "resend_cut" | "send" | "send_cut" => {
            super::send::handle(&ctx, action, &parts).await?;
        }
        // Clipping & circles
        "clip" | "clip_cut" | "circle" | "circle_cut" | "clip_cancel" | "circle_subs" | "circle_sub_lang" | "ts"
        | "dur" | "gif" | "gif_cut" => {
            super::clipping::handle(&ctx, action, &parts).await?;
        }
        // Speed control
        "speed" | "speed_cut" | "apply_speed" | "apply_speed_cut" => {
            super::speed::handle(&ctx, action, &parts).await?;
        }
        // Voice, subtitles, lyrics
        "subtitles" | "burn_subs" | "burn_subs_lang" | "voice" | "voice_dur" | "lyrics" | "lyr_cap" | "lyrics_send" => {
            super::voice_lyrics::handle(&ctx, action, &parts).await?;
        }
        // Cover (photo/gif/clip from video source)
        "cover" | "cover_do" => {
            super::cover::handle(&ctx, action, &parts).await?;
        }
        // Categories
        "setcat" | "savecat" | "newcat" => {
            super::categories::handle(&ctx, action, &parts).await?;
        }
        // Simple actions
        "cancel" | "close" => {
            bot.delete_message(chat_id, message_id).await?;
        }
        _ => {}
    }

    Ok(())
}
