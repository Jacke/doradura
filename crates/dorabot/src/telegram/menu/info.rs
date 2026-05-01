//! Info-feature callback handling — `info:` prefix.
//!
//! Non-download actions on a previewed URL: max-resolution thumbnail,
//! geo-availability check, full metadata card. Each action reads from
//! `EXTENDED_METADATA_CACHE` (populated alongside `PREVIEW_CACHE` during
//! the preview fetch); cache miss falls through to a fresh
//! `yt-dlp --dump-json` invocation.
//!
//! Callback shape: `info:{action}:{url_id}` where `action ∈
//! {menu, thumb, geo, meta}` and `url_id` is the short hash resolved
//! via `cache::get_url`.
//!
//! v0.51.0-alpha.2: scaffolding — `menu` opens the submenu, sub-actions
//! show a "coming soon" answer until alpha.3 implements them.

use std::sync::Arc;
use std::time::Duration;

use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, InputFile, ParseMode};

use crate::core::utils::escape_markdown_v2;
use crate::storage::SharedStorage;
use crate::storage::cache;
use crate::storage::db::DbPool;
use crate::telegram::Bot;
use crate::telegram::cache::{EXTENDED_METADATA_CACHE, PREVIEW_CACHE};
use crate::telegram::types::ExtendedMetadata;
use doracore::core::country::format_country_list;
use doracore::core::utils::format_bytes;
use doracore::download::ringtone::sanitize_filename;
use doracore::download::thumbnail::{ImageFormat, detect_image_format};

/// Entry point for `info:*` callback queries.
///
/// Parses `info:{action}:{url_id}` and dispatches. All sub-actions
/// answer the callback query so the spinner clears.
pub async fn handle_info_callback(
    bot: &Bot,
    callback_id: teloxide::types::CallbackQueryId,
    chat_id: ChatId,
    message_id: teloxide::types::MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
    shared_storage: Arc<SharedStorage>,
) -> ResponseResult<()> {
    let parts: Vec<&str> = data.splitn(3, ':').collect();
    if parts.len() != 3 {
        let _ = bot.answer_callback_query(callback_id).await;
        return Ok(());
    }
    let action = parts[1];
    let url_id = parts[2];

    match action {
        "menu" => {
            let _ = bot.answer_callback_query(callback_id).await;
            show_info_menu(bot, chat_id, message_id, url_id).await;
        }
        "geo" => {
            let _ = bot.answer_callback_query(callback_id).await;
            send_geo_card(bot, chat_id, url_id, &db_pool, &shared_storage).await;
        }
        "thumb" => {
            let _ = bot.answer_callback_query(callback_id).await;
            send_max_thumbnail(bot, chat_id, url_id, &db_pool, &shared_storage).await;
        }
        "meta" => {
            let _ = bot.answer_callback_query(callback_id).await;
            send_metadata_card(bot, chat_id, url_id, &db_pool, &shared_storage).await;
        }
        _ => {
            let _ = bot.answer_callback_query(callback_id).await;
        }
    }

    Ok(())
}

/// Resolve `url_id` to a `String` URL via the shared id↔url cache.
/// Returns `None` if the entry expired or is missing — caller should
/// surface a friendly "preview expired, paste again" message.
async fn resolve_url(url_id: &str, db_pool: &DbPool, shared_storage: &SharedStorage) -> Option<String> {
    cache::get_url(db_pool, Some(shared_storage), url_id).await
}

/// Pull `ExtendedMetadata` for a URL, prefer cache, fall back to a fresh
/// `--dump-json` invocation by re-running the preview metadata fetch
/// (which populates the cache as a side-effect).
async fn fetch_extended(url: &str) -> Option<ExtendedMetadata> {
    EXTENDED_METADATA_CACHE.get(url).await
}

/// Render and send the geo-availability card. Reads from
/// `EXTENDED_METADATA_CACHE`; if the entry is gone (TTL eviction), falls
/// back to a friendly "preview expired" reply rather than re-running
/// yt-dlp on a stale request.
async fn send_geo_card(bot: &Bot, chat_id: ChatId, url_id: &str, db_pool: &DbPool, shared_storage: &SharedStorage) {
    let Some(url) = resolve_url(url_id, db_pool, shared_storage).await else {
        let _ = bot
            .send_message(chat_id, "⏳ Превью устарело — пришли ссылку ещё раз")
            .await;
        return;
    };

    let Some(ext) = fetch_extended(&url).await else {
        let _ = bot
            .send_message(chat_id, "⏳ Метаданные устарели — пришли ссылку ещё раз")
            .await;
        return;
    };

    let availability = ext.availability.as_deref().unwrap_or("public");
    let geo_block_line = if ext.geo_block {
        "✅ Да".to_string()
    } else {
        "❌ Нет".to_string()
    };

    let mut card = String::new();
    card.push_str("🌍 *Доступность*\n\n");
    card.push_str(&format!("Статус: `{}`\n", escape_markdown_v2(availability)));
    card.push_str(&format!("Гео\\-блокировка: {}\n", escape_markdown_v2(&geo_block_line)));
    if !ext.blocked_countries.is_empty() {
        card.push_str(&format!(
            "Заблокировано в: {}\n",
            escape_markdown_v2(&format_country_list(&ext.blocked_countries))
        ));
    } else if !ext.geo_block {
        card.push_str("Доступно везде ✅\n");
    }

    if let Err(e) = bot.send_message(chat_id, card).parse_mode(ParseMode::MarkdownV2).await {
        log::warn!(
            "info::send_geo_card: send failed ({:?}); retrying without parse_mode",
            e
        );
        let _ = bot
            .send_message(chat_id, "🌍 Не удалось отрендерить карточку — попробуйте позже")
            .await;
    }
}

/// Render the Info submenu — 3 action buttons + Cancel.
///
/// Edits the existing preview message's keyboard (no new message
/// spawned — keeps the chat clean).
async fn show_info_menu(bot: &Bot, chat_id: ChatId, message_id: teloxide::types::MessageId, url_id: &str) {
    let buttons = vec![
        vec![InlineKeyboardButton::callback(
            "🖼 Скачать обложку (max-res)",
            format!("info:thumb:{}", url_id),
        )],
        vec![InlineKeyboardButton::callback(
            "🌍 Доступность по странам",
            format!("info:geo:{}", url_id),
        )],
        vec![InlineKeyboardButton::callback(
            "📋 Полные метаданные",
            format!("info:meta:{}", url_id),
        )],
        vec![InlineKeyboardButton::callback(
            "❌ Отмена",
            format!("pv:cancel:{}", url_id),
        )],
    ];
    let keyboard = InlineKeyboardMarkup::new(buttons);

    if let Err(e) = bot
        .edit_message_reply_markup(chat_id, message_id)
        .reply_markup(keyboard)
        .await
    {
        log::warn!("info: failed to edit submenu keyboard: {:?}", e);
    }
}

/// Photo-vs-document threshold — Telegram silently re-encodes anything sent
/// via `send_photo` to ≤2560 px wide. Above this size we send as document
/// to preserve the original bytes.
const PHOTO_THRESHOLD_BYTES: u64 = 9 * 1024 * 1024;

/// HTTP timeout for the standalone thumbnail download — matches the pattern
/// used by the existing `send.rs` thumbnail fetch.
const THUMBNAIL_FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// Download the widest thumbnail the source exposes and forward it to the
/// chat. Goes through `send_photo` for ≤9 MB JPG/PNG/WebP; falls back to
/// `send_document` for larger files (Telegram re-compresses anything bigger
/// inside `send_photo`).
async fn send_max_thumbnail(
    bot: &Bot,
    chat_id: ChatId,
    url_id: &str,
    db_pool: &DbPool,
    shared_storage: &SharedStorage,
) {
    let Some(url) = resolve_url(url_id, db_pool, shared_storage).await else {
        let _ = bot
            .send_message(chat_id, "⏳ Превью устарело — пришли ссылку ещё раз")
            .await;
        return;
    };

    let Some(ext) = fetch_extended(&url).await else {
        let _ = bot
            .send_message(chat_id, "⏳ Метаданные устарели — пришли ссылку ещё раз")
            .await;
        return;
    };

    let thumb_url = match ext.thumbnail_max_url {
        Some(u) if !u.is_empty() => u,
        _ => {
            let _ = bot.send_message(chat_id, "🖼 У источника нет обложки").await;
            return;
        }
    };

    // Reuse the global reqwest client so we get the same timeout / TLS
    // settings as the rest of the bot.
    let client = reqwest::Client::builder()
        .timeout(THUMBNAIL_FETCH_TIMEOUT)
        .build()
        .unwrap_or_default();

    let bytes = match client.get(&thumb_url).send().await {
        Ok(resp) if resp.status().is_success() => match resp.bytes().await {
            Ok(b) => b.to_vec(),
            Err(e) => {
                log::warn!("info::send_max_thumbnail: read body failed: {}", e);
                let _ = bot.send_message(chat_id, "🖼 Не удалось скачать обложку").await;
                return;
            }
        },
        Ok(resp) => {
            log::warn!("info::send_max_thumbnail: HTTP {}", resp.status());
            let _ = bot
                .send_message(chat_id, "🖼 Источник вернул ошибку — обложка недоступна")
                .await;
            return;
        }
        Err(e) => {
            log::warn!("info::send_max_thumbnail: request failed: {}", e);
            let _ = bot.send_message(chat_id, "🖼 Не удалось связаться с источником").await;
            return;
        }
    };

    let format = detect_image_format(&bytes);
    let ext_str = match format {
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Png => "png",
        ImageFormat::WebP => "webp",
        ImageFormat::Unknown => "jpg",
    };

    // Pull a friendly base name from the preview cache (already populated for
    // any Info-callable URL) — falls back to a generic name if absent.
    let base_name = PREVIEW_CACHE
        .get(&url)
        .await
        .map(|p| sanitize_filename(&p.title))
        .unwrap_or_else(|| "thumbnail".to_string());
    let filename = format!("{}_thumbnail.{}", base_name, ext_str);

    let size = bytes.len() as u64;
    let caption = format!("🖼 Cover ({})", format_bytes(size));

    let input_file = InputFile::memory(bytes).file_name(filename);

    let result = if size <= PHOTO_THRESHOLD_BYTES && format != ImageFormat::Unknown {
        bot.send_photo(chat_id, input_file).caption(caption).await.map(drop)
    } else {
        bot.send_document(chat_id, input_file).caption(caption).await.map(drop)
    };

    if let Err(e) = result {
        log::warn!("info::send_max_thumbnail: send failed: {:?}", e);
        let _ = bot
            .send_message(chat_id, "🖼 Telegram отклонил обложку — попробуйте позже")
            .await;
    }
}

/// Render and send the full-metadata card. Sent as a Markdown message when
/// short, or as a `.txt` document when above Telegram's 4096-char limit.
async fn send_metadata_card(
    bot: &Bot,
    chat_id: ChatId,
    url_id: &str,
    db_pool: &DbPool,
    shared_storage: &SharedStorage,
) {
    let Some(url) = resolve_url(url_id, db_pool, shared_storage).await else {
        let _ = bot
            .send_message(chat_id, "⏳ Превью устарело — пришли ссылку ещё раз")
            .await;
        return;
    };

    let Some(ext) = fetch_extended(&url).await else {
        let _ = bot
            .send_message(chat_id, "⏳ Метаданные устарели — пришли ссылку ещё раз")
            .await;
        return;
    };
    let preview = PREVIEW_CACHE.get(&url).await;

    // Build the Markdown card. If it ends up over the Telegram message
    // limit (4096 chars), we'll fall back to a plain-text document.
    let card_md = render_metadata_markdown(&ext, preview.as_ref());

    if card_md.len() <= 3500 {
        // Leave a margin under the 4096 limit for footers/escapes.
        if let Err(e) = bot
            .send_message(chat_id, &card_md)
            .parse_mode(ParseMode::MarkdownV2)
            .await
        {
            log::warn!(
                "info::send_metadata_card: MarkdownV2 send failed ({:?}); falling back to plain doc",
                e
            );
            send_metadata_as_doc(bot, chat_id, &ext, preview.as_ref()).await;
        }
    } else {
        send_metadata_as_doc(bot, chat_id, &ext, preview.as_ref()).await;
    }
}

/// Render the metadata card as MarkdownV2.
fn render_metadata_markdown(
    ext: &ExtendedMetadata,
    preview: Option<&crate::telegram::types::PreviewMetadata>,
) -> String {
    let mut s = String::new();
    s.push_str("📋 *Метаданные*\n\n");

    if let Some(p) = preview {
        if !p.title.trim().is_empty() {
            s.push_str(&format!("*{}*\n", escape_markdown_v2(&p.title)));
        }
        if !p.artist.trim().is_empty() {
            s.push_str(&format!("📺 {}\n", escape_markdown_v2(&p.artist)));
        }
    }
    if let Some(ref ch) = ext.channel_url {
        s.push_str(&format!("🔗 {}\n", escape_markdown_v2(ch)));
    }
    if let Some(ref date) = ext.upload_date {
        // YYYYMMDD → YYYY-MM-DD
        let pretty = if date.len() == 8 {
            format!("{}-{}-{}", &date[..4], &date[4..6], &date[6..8])
        } else {
            date.clone()
        };
        s.push_str(&format!("📅 {}\n", escape_markdown_v2(&pretty)));
    }

    let stats: Vec<String> = [
        ext.view_count.map(|v| format!("👁 {} просм.", v)),
        ext.like_count.map(|v| format!("👍 {}", v)),
        ext.comment_count.map(|v| format!("💬 {}", v)),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !stats.is_empty() {
        s.push_str(&format!("📊 {}\n", escape_markdown_v2(&stats.join(" · "))));
    }

    if !ext.tags.is_empty() {
        let preview = ext.tags.iter().take(15).cloned().collect::<Vec<_>>().join(", ");
        s.push_str(&format!("\n🏷 _{}_\n", escape_markdown_v2(&preview)));
    }
    if !ext.categories.is_empty() {
        s.push_str(&format!("📂 {}\n", escape_markdown_v2(&ext.categories.join(", "))));
    }

    if let Some(ref desc) = ext.description_full {
        let trimmed = desc.trim();
        if !trimmed.is_empty() {
            // Cap description in the inline card; full text always available
            // via the document fallback path.
            let snippet = if trimmed.chars().count() > 800 {
                let mut t: String = trimmed.chars().take(800).collect();
                t.push('…');
                t
            } else {
                trimmed.to_string()
            };
            s.push_str(&format!("\n📝 {}\n", escape_markdown_v2(&snippet)));
        }
    }

    s
}

/// Render the metadata card as a plain-text `.txt` document and send it.
/// Used when the Markdown card exceeds Telegram's 4096-char limit.
async fn send_metadata_as_doc(
    bot: &Bot,
    chat_id: ChatId,
    ext: &ExtendedMetadata,
    preview: Option<&crate::telegram::types::PreviewMetadata>,
) {
    let mut s = String::new();
    s.push_str("=== Metadata ===\n\n");
    if let Some(p) = preview {
        if !p.title.trim().is_empty() {
            s.push_str(&format!("Title: {}\n", p.title));
        }
        if !p.artist.trim().is_empty() {
            s.push_str(&format!("Channel: {}\n", p.artist));
        }
    }
    if let Some(ref ch) = ext.channel_url {
        s.push_str(&format!("Channel URL: {}\n", ch));
    }
    if let Some(ref date) = ext.upload_date {
        s.push_str(&format!("Upload date: {}\n", date));
    }
    if let Some(v) = ext.view_count {
        s.push_str(&format!("Views: {}\n", v));
    }
    if let Some(v) = ext.like_count {
        s.push_str(&format!("Likes: {}\n", v));
    }
    if let Some(v) = ext.comment_count {
        s.push_str(&format!("Comments: {}\n", v));
    }
    if !ext.categories.is_empty() {
        s.push_str(&format!("Categories: {}\n", ext.categories.join(", ")));
    }
    if !ext.tags.is_empty() {
        s.push_str(&format!("\nTags ({}):\n  {}\n", ext.tags.len(), ext.tags.join(", ")));
    }
    if let Some(ref a) = ext.availability {
        s.push_str(&format!("\nAvailability: {}\n", a));
    }
    s.push_str(&format!("Geo-blocked: {}\n", if ext.geo_block { "yes" } else { "no" }));
    if !ext.blocked_countries.is_empty() {
        s.push_str(&format!("Blocked in: {}\n", ext.blocked_countries.join(", ")));
    }
    if let Some(ref desc) = ext.description_full {
        s.push_str("\n=== Description ===\n");
        s.push_str(desc);
        s.push('\n');
    }

    let base = preview
        .map(|p| sanitize_filename(&p.title))
        .unwrap_or_else(|| "metadata".to_string());
    let filename = format!("{}_metadata.txt", base);
    let input = InputFile::memory(s.into_bytes()).file_name(filename);

    if let Err(e) = bot.send_document(chat_id, input).await {
        log::warn!("info::send_metadata_as_doc: send_document failed: {:?}", e);
        let _ = bot
            .send_message(chat_id, "📋 Не удалось отправить метаданные — попробуйте позже")
            .await;
    }
}
