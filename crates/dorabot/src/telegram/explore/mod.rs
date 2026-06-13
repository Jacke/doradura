//! Inline Explore hub: renders the timeline (Recent tab) and handles
//! tab/page/resend callbacks. Discovery tabs (Trending/Subscriptions) are
//! placeholders until sub-projects C/B land.

pub mod render;

use std::sync::Arc;

use teloxide::prelude::*;
use teloxide::types::{ChatId, FileId, InlineKeyboardMarkup, InputFile, ParseMode};
use unic_langid::LanguageIdentifier;

use crate::i18n;
use crate::storage::SharedStorage;
use crate::telegram::Bot;
use doracore::explore::timeline::{self, BucketLabel, MediaKind, TimelinePage};

use render::{render_timeline_keyboard, render_timeline_text};

/// Dispatch `exp:*` callbacks: tab switch, pagination, resend-by-history-id.
///
/// `exp:tab:recent` / `exp:page:recent:{n}` re-render the timeline in place.
/// `exp:tab:trending` / `exp:tab:subs` answer with a "coming soon" toast.
/// `exp:rs:{id}` re-sends a past download (instant via file_id, else URL).
pub async fn handle_explore_callback(
    bot: Bot,
    q: CallbackQuery,
    data: &str,
    storage: Arc<SharedStorage>,
) -> anyhow::Result<()> {
    let user_id = i64::try_from(q.from.id.0).unwrap_or(0);
    let parts: Vec<&str> = data.split(':').collect();

    match parts.as_slice() {
        ["exp", "tab", "recent"] => show_recent(&bot, &q, &storage, user_id, 0).await,
        ["exp", "page", "recent", p] => {
            let page = p.parse().unwrap_or(0);
            show_recent(&bot, &q, &storage, user_id, page).await
        }
        ["exp", "tab", _other] => {
            let lang = i18n::user_lang_from_storage(&storage, user_id).await;
            bot.answer_callback_query(q.id.clone())
                .text(i18n::t(&lang, "explore_soon"))
                .await?;
            Ok(())
        }
        ["exp", "rs", id] => {
            let hist_id = id.parse().unwrap_or(0);
            resend_entry(&bot, &q, &storage, user_id, hist_id).await
        }
        _ => {
            // `exp:noop` (the page-label button) and any unknown shape just
            // clear the spinner.
            bot.answer_callback_query(q.id.clone()).await?;
            Ok(())
        }
    }
}

/// Localized header for a date bucket.
fn bucket_header(lang: &LanguageIdentifier, label: BucketLabel) -> String {
    let key = match label {
        BucketLabel::Today => "explore_bucket_today",
        BucketLabel::Yesterday => "explore_bucket_yesterday",
        BucketLabel::ThisWeek => "explore_bucket_week",
        BucketLabel::ThisMonth => "explore_bucket_month",
        BucketLabel::Earlier => "explore_bucket_earlier",
    };
    i18n::t(lang, key)
}

/// Build the timeline message (text + keyboard) for `page`.
///
/// Shared by [`show_recent`] (which edits the callback's message in place) and
/// [`show_recent_fresh`] (which sends a brand-new message). Returns the rendered
/// HTML text and the timeline keyboard, or an error if the page can't be built
/// from storage.
async fn render_recent(
    storage: &Arc<SharedStorage>,
    user_id: i64,
    page: u32,
) -> anyhow::Result<(String, InlineKeyboardMarkup)> {
    let lang = i18n::user_lang_from_storage(storage, user_id).await;

    let page: TimelinePage = timeline::build_timeline_page(storage, user_id, page, chrono::Utc::now()).await?;

    // HTML parse mode: inline-button labels are plain text, but the rich card
    // body uses HTML (cleaner than MarkdownV2 — no escape-soup).
    let html = teloxide::utils::html::escape;
    let title = format!(
        "<b>{}</b>  ·  {}",
        html(&i18n::t(&lang, "explore_title")),
        page.total_entries
    );
    let empty = i18n::t(&lang, "explore_empty");
    let header = |label: BucketLabel| bucket_header(&lang, label);
    let text = render_timeline_text(&page, &title, &empty, &header, &|s| html(s));

    let page_label = {
        let args = doracore::fluent_args!("page" => page.page + 1, "total" => page.total_pages);
        i18n::t_args(&lang, "explore_page", &args)
    };
    let keyboard = render_timeline_keyboard(
        &page,
        &i18n::t(&lang, "explore_tab_recent"),
        &i18n::t(&lang, "explore_tab_trending"),
        &i18n::t(&lang, "explore_tab_subs"),
        &page_label,
    );

    Ok((text, keyboard))
}

/// Build the timeline message for `page` and edit the callback's message in
/// place. Used by tab-switch and pagination callbacks.
async fn show_recent(
    bot: &Bot,
    q: &CallbackQuery,
    storage: &Arc<SharedStorage>,
    user_id: i64,
    page: u32,
) -> anyhow::Result<()> {
    let (text, keyboard) = match render_recent(storage, user_id, page).await {
        Ok(rendered) => rendered,
        Err(e) => {
            log::error!("explore: build_timeline_page failed for {}: {}", user_id, e);
            let lang = i18n::user_lang_from_storage(storage, user_id).await;
            bot.answer_callback_query(q.id.clone())
                .text(i18n::t(&lang, "explore_load_failed"))
                .await?;
            return Ok(());
        }
    };

    // Edit the message the callback was invoked from.
    if let Some(msg) = q.message.as_ref() {
        let chat_id = msg.chat().id;
        let message_id = msg.id();
        if let Err(e) = bot
            .edit_message_text(chat_id, message_id, text)
            .parse_mode(ParseMode::Html)
            .reply_markup(keyboard)
            .await
        {
            log::warn!("explore: edit_message_text failed: {}", e);
        }
    }

    bot.answer_callback_query(q.id.clone()).await?;
    Ok(())
}

/// Build the timeline (page 0) and send it as a NEW message. Used by the
/// `/explore` command and the main-menu Explore button, which have no callback
/// message to edit.
pub async fn show_recent_fresh(
    bot: &Bot,
    chat_id: ChatId,
    storage: &Arc<SharedStorage>,
    user_id: i64,
) -> anyhow::Result<()> {
    match render_recent(storage, user_id, 0).await {
        Ok((text, keyboard)) => {
            bot.send_message(chat_id, text)
                .parse_mode(ParseMode::Html)
                .reply_markup(keyboard)
                .await?;
        }
        Err(e) => {
            log::error!("explore: build_timeline_page failed for {}: {}", user_id, e);
            let lang = i18n::user_lang_from_storage(storage, user_id).await;
            bot.send_message(chat_id, i18n::t(&lang, "explore_load_failed")).await?;
        }
    }
    Ok(())
}

/// Re-send a past download by history id. Instant via cached Telegram file_id;
/// on an expired file reference / missing file_id, fall back to the stored URL
/// so the user can re-download by tapping it.
async fn resend_entry(
    bot: &Bot,
    q: &CallbackQuery,
    storage: &Arc<SharedStorage>,
    user_id: i64,
    hist_id: i64,
) -> anyhow::Result<()> {
    let lang = i18n::user_lang_from_storage(storage, user_id).await;
    let chat_id = ChatId(user_id);

    let entry = storage.get_download_history_entry(user_id, hist_id).await?;
    let Some(entry) = entry else {
        return fallback_resend(bot, q, &lang, None).await;
    };

    let media = timeline::media_kind_from_format(&entry.format);

    if let Some(fid) = entry.file_id.clone() {
        let input = InputFile::file_id(FileId(fid));
        let send_result = match media {
            MediaKind::Audio => bot.send_audio(chat_id, input).await.map(|_| ()),
            MediaKind::Video => bot.send_video(chat_id, input).await.map(|_| ()),
            MediaKind::VideoNote => bot.send_video_note(chat_id, input).await.map(|_| ()),
            MediaKind::Gif => bot.send_animation(chat_id, input).await.map(|_| ()),
            MediaKind::Other => bot.send_document(chat_id, input).await.map(|_| ()),
        };

        match send_result {
            Ok(()) => {
                bot.answer_callback_query(q.id.clone())
                    .text(i18n::t(&lang, "explore_resent"))
                    .await?;
                return Ok(());
            }
            Err(e) => {
                log::warn!(
                    "explore: file_id resend failed for entry {} ({}), falling back to URL",
                    hist_id,
                    e
                );
            }
        }
    }

    fallback_resend(bot, q, &lang, Some(&entry.url)).await
}

/// No cached file_id (or it expired / entry missing): clear the spinner and,
/// when a URL is known, send it so the user can re-download by tapping the link.
async fn fallback_resend(
    bot: &Bot,
    q: &CallbackQuery,
    lang: &LanguageIdentifier,
    url: Option<&str>,
) -> anyhow::Result<()> {
    let chat_id = ChatId(i64::try_from(q.from.id.0).unwrap_or(0));
    if let Some(url) = url {
        let _ = bot.send_message(chat_id, url.to_string()).await;
        bot.answer_callback_query(q.id.clone())
            .text(i18n::t(lang, "explore_resent"))
            .await?;
    } else {
        bot.answer_callback_query(q.id.clone())
            .text(i18n::t(lang, "explore_load_failed"))
            .await?;
    }
    Ok(())
}
