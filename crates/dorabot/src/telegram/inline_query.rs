//! Inline query handler for Vlipsy video reaction search.
//!
//! When users type `@botname query` in any chat, this handler searches Vlipsy
//! and returns video results that can be sent inline.

use crate::vlipsy::VlipsyClient;
use teloxide::prelude::*;
use teloxide::types::{InlineQueryResult, InlineQueryResultVideo};

const RESULTS_PER_PAGE: u32 = 10;

/// Handle inline queries by searching Vlipsy for video reactions.
///
/// - Empty query → trending clips
/// - Non-empty query → search
/// - Pagination via `query.offset`
pub async fn handle_inline_query(bot: crate::telegram::Bot, query: InlineQuery) -> ResponseResult<()> {
    let client = match VlipsyClient::new() {
        Some(c) => c,
        None => {
            // Vlipsy not configured — return empty results
            bot.answer_inline_query(query.id, Vec::<InlineQueryResult>::new())
                .cache_time(300)
                .await?;
            return Ok(());
        }
    };

    let offset: u32 = query.offset.parse().unwrap_or(0);
    let search_query = query.query.trim().to_string();

    let api_result = if search_query.is_empty() {
        client.trending(RESULTS_PER_PAGE, offset).await
    } else {
        client.search(&search_query, RESULTS_PER_PAGE, offset).await
    };

    let response = match api_result {
        Ok(r) => r,
        Err(e) => {
            log::error!("Vlipsy inline query error: {}", e);
            bot.answer_inline_query(query.id, Vec::<InlineQueryResult>::new())
                .cache_time(60)
                .await?;
            return Ok(());
        }
    };

    let mut results: Vec<InlineQueryResult> = Vec::new();
    let mp4_mime: mime::Mime = "video/mp4".parse().unwrap();

    for vlip in &response.results {
        if let Some(mp4_url) = vlip.mp4_url() {
            let title = vlip.display_title().to_string();
            let thumb_url = vlip.thumb_url().unwrap_or(mp4_url);

            let Ok(video_url) = mp4_url.parse() else { continue };
            let Ok(thumb) = thumb_url.parse() else { continue };

            let result = InlineQueryResultVideo::new(vlip.id.clone(), video_url, mp4_mime.clone(), thumb, title);

            results.push(InlineQueryResult::Video(result));
        }
    }

    // Calculate next offset for pagination
    let next_offset = if results.len() == RESULTS_PER_PAGE as usize {
        (offset + RESULTS_PER_PAGE).to_string()
    } else {
        String::new()
    };

    bot.answer_inline_query(query.id, results)
        .cache_time(300)
        .is_personal(false)
        .next_offset(next_offset)
        .await?;

    Ok(())
}
