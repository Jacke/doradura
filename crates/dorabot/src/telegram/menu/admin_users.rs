use crate::storage::SharedStorage;
use crate::telegram::Bot;
use crate::telegram::admin;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, MessageId};

const PAGE_SIZE: usize = 8;
const ADMIN_SEARCH_PROMPT_KIND: &str = "admin_user_search";
const ADMIN_SEARCH_TTL_SECS: i64 = 60;

// --- Search sessions ---

/// Check if an admin is currently in search mode.
pub async fn is_admin_searching(shared_storage: &Arc<SharedStorage>, admin_id: i64) -> bool {
    shared_storage
        .get_prompt_session(admin_id, ADMIN_SEARCH_PROMPT_KIND)
        .await
        .ok()
        .flatten()
        .is_some()
}

async fn set_admin_searching(shared_storage: &Arc<SharedStorage>, admin_id: i64) {
    let _ = shared_storage
        .upsert_prompt_session(admin_id, ADMIN_SEARCH_PROMPT_KIND, "", ADMIN_SEARCH_TTL_SECS)
        .await;
}

async fn clear_admin_searching(shared_storage: &Arc<SharedStorage>, admin_id: i64) {
    let _ = shared_storage
        .delete_prompt_session(admin_id, ADMIN_SEARCH_PROMPT_KIND)
        .await;
}

fn preserves_search_mode(action: Option<&str>) -> bool {
    matches!(action, Some("s"))
}

// --- Filter ---

#[derive(Clone, Copy, PartialEq, Default)]
pub enum Filter {
    #[default]
    All,
    Free,
    Premium,
    Vip,
    Blocked,
}

impl Filter {
    fn from_code(s: &str) -> Self {
        match s {
            "f" => Filter::Free,
            "p" => Filter::Premium,
            "v" => Filter::Vip,
            "b" => Filter::Blocked,
            _ => Filter::All,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            Filter::All => "a",
            Filter::Free => "f",
            Filter::Premium => "p",
            Filter::Vip => "v",
            Filter::Blocked => "b",
        }
    }

    fn db_filter(&self) -> Option<&'static str> {
        match self {
            Filter::All => None,
            Filter::Free => Some("free"),
            Filter::Premium => Some("premium"),
            Filter::Vip => Some("vip"),
            Filter::Blocked => Some("blocked"),
        }
    }
}

fn cb(label: impl Into<String>, data: impl Into<String>) -> teloxide::types::InlineKeyboardButton {
    crate::telegram::cb(label, data)
}

fn username_display(user: &crate::storage::db::User) -> String {
    user.username
        .as_ref()
        .map(|u| format!("@{}", u))
        .unwrap_or_else(|| format!("ID:{}", user.telegram_id))
}

fn username_short(user: &crate::storage::db::User) -> String {
    user.username
        .as_deref()
        .map(|u| format!("@{}", u))
        .unwrap_or_else(|| user.telegram_id.to_string())
}

// --- User list ---

pub async fn show_user_list(
    bot: &Bot,
    chat_id: ChatId,
    msg_id: Option<MessageId>,
    shared_storage: &Arc<SharedStorage>,
    page: usize,
    filter: Filter,
) -> anyhow::Result<()> {
    let counts = shared_storage.get_user_counts().await?;

    let offset = page * PAGE_SIZE;
    let (page_users, filtered_total) = shared_storage
        .get_users_paginated(filter.db_filter(), offset, PAGE_SIZE)
        .await?;

    let total_pages = filtered_total.max(1).div_ceil(PAGE_SIZE);
    let page = page.min(total_pages.saturating_sub(1));

    let mut text = format!(
        "🔧 Admin: Users ({})\n🆓 {}  ⭐ {}  👑 {}  🚫 {}\n\n",
        counts.total, counts.free, counts.premium, counts.vip, counts.blocked
    );

    let start = page * PAGE_SIZE;
    for (i, user) in page_users.iter().enumerate() {
        let blocked_mark = if user.is_blocked { " 🚫" } else { "" };
        text.push_str(&format!(
            "{}. {} {}{} ({})\n",
            start + i + 1,
            user.plan.emoji(),
            username_display(user),
            blocked_mark,
            user.telegram_id
        ));
    }

    let f = filter.code();
    let mut keyboard_rows: Vec<Vec<teloxide::types::InlineKeyboardButton>> = Vec::new();

    // User buttons (2 per row)
    let mut row = Vec::new();
    for user in &page_users {
        let label = format!("{} {}", user.plan.emoji(), username_short(user));
        row.push(cb(label, format!("au:u:{}", user.telegram_id)));
        if row.len() == 2 {
            keyboard_rows.push(std::mem::take(&mut row));
        }
    }
    if !row.is_empty() {
        keyboard_rows.push(row);
    }

    // Pagination
    let mut nav_row = Vec::new();
    if page > 0 {
        nav_row.push(cb("◀", format!("au:l:{}:{}", page - 1, f)));
    }
    nav_row.push(cb(format!("{}/{}", page + 1, total_pages), "au:noop"));
    if page + 1 < total_pages {
        nav_row.push(cb("▶", format!("au:l:{}:{}", page + 1, f)));
    }
    keyboard_rows.push(nav_row);

    // Filter row
    let filter_labels = [("All", "a"), ("🆓", "f"), ("⭐", "p"), ("👑", "v"), ("🚫", "b")];
    let filter_row: Vec<_> = filter_labels
        .iter()
        .map(|(label, code)| {
            let display = if *code == f {
                format!("[{}]", label)
            } else {
                label.to_string()
            };
            cb(display, format!("au:l:0:{}", code))
        })
        .collect();
    keyboard_rows.push(filter_row);

    // Search button
    keyboard_rows.push(vec![cb("🔍 Search", "au:s")]);

    let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

    if let Some(mid) = msg_id {
        if let Err(e) = bot.edit_message_text(chat_id, mid, &text).reply_markup(keyboard).await {
            log::warn!("Failed to edit admin user list: {}", e);
        }
    } else {
        bot.send_message(chat_id, &text).reply_markup(keyboard).await?;
    }

    Ok(())
}

// --- User detail ---

async fn show_user_detail(
    bot: &Bot,
    chat_id: ChatId,
    msg_id: MessageId,
    shared_storage: &Arc<SharedStorage>,
    target_uid: i64,
) -> anyhow::Result<()> {
    let user = match shared_storage.get_user(target_uid).await? {
        Some(u) => u,
        None => {
            log::warn!("Admin: user {} not found", target_uid);
            let _ = bot.edit_message_text(chat_id, msg_id, "❌ User not found").await;
            return Ok(());
        }
    };

    let expires_info = user
        .subscription_expires_at
        .as_ref()
        .map(|e| format!("  📅 until {}", e))
        .unwrap_or_default();

    let status = if user.is_blocked { "🚫 Blocked" } else { "✅ Active" };

    let text = format!(
        "👤 {}\n🆔 {}\n📋 {} {}{}  \n{}\n\n⚙️ fmt:{} | vid:{} | aud:{}\n🌐 {} | 📊 {}",
        username_display(&user),
        user.telegram_id,
        user.plan.emoji(),
        user.plan.display_name(),
        expires_info,
        status,
        user.download_format,
        user.video_quality,
        user.audio_bitrate,
        user.language,
        user.progress_bar_style,
    );

    let uid = user.telegram_id;
    let mut rows: Vec<Vec<teloxide::types::InlineKeyboardButton>> = Vec::new();

    rows.push(vec![
        cb("🆓 Free", format!("au:sp:{}:f", uid)),
        cb("⭐ Prem", format!("au:sp:{}:p", uid)),
        cb("👑 VIP", format!("au:sp:{}:v", uid)),
    ]);

    let block_label = if user.is_blocked { "✅ Unblock" } else { "🚫 Block" };
    rows.push(vec![cb(block_label, format!("au:b:{}", uid))]);
    rows.push(vec![cb("⚙️ Settings", format!("au:st:{}", uid))]);
    rows.push(vec![cb("🔙 Back", "au:l:0:a")]);

    let keyboard = InlineKeyboardMarkup::new(rows);
    if let Err(e) = bot
        .edit_message_text(chat_id, msg_id, text)
        .reply_markup(keyboard)
        .await
    {
        log::warn!("Failed to edit user detail: {}", e);
    }

    Ok(())
}

// --- Set plan ---

async fn handle_set_plan(
    bot: &Bot,
    chat_id: ChatId,
    msg_id: MessageId,
    shared_storage: &Arc<SharedStorage>,
    uid: i64,
    plan_code: &str,
) -> anyhow::Result<()> {
    let plan = match plan_code {
        "f" => "free",
        "p" => "premium",
        "v" => "vip",
        _ => return Ok(()),
    };

    if plan == "free" {
        shared_storage.update_user_plan_with_expiry(uid, plan, None).await?;
        notify_user_plan_change(bot, uid, plan).await;
        show_user_detail(bot, chat_id, msg_id, shared_storage, uid).await?;
        return Ok(());
    }

    let plan_emoji = if plan == "premium" { "⭐" } else { "👑" };
    let plan_name = if plan == "premium" { "Premium" } else { "VIP" };
    let text = format!("📅 Set {} {} duration for user {}:", plan_emoji, plan_name, uid);

    let pc = plan_code;
    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![
            cb("7 days", format!("au:se:{}:{}:7", uid, pc)),
            cb("30 days", format!("au:se:{}:{}:30", uid, pc)),
        ],
        vec![
            cb("90 days", format!("au:se:{}:{}:90", uid, pc)),
            cb("365 days", format!("au:se:{}:{}:365", uid, pc)),
        ],
        vec![cb("♾ Unlimited", format!("au:se:{}:{}:0", uid, pc))],
        vec![cb("🔙 Back", format!("au:u:{}", uid))],
    ]);

    if let Err(e) = bot
        .edit_message_text(chat_id, msg_id, text)
        .reply_markup(keyboard)
        .await
    {
        log::warn!("Failed to edit expiry selector: {}", e);
    }
    Ok(())
}

// --- Set expiry ---

async fn handle_set_expiry(
    bot: &Bot,
    chat_id: ChatId,
    msg_id: MessageId,
    shared_storage: &Arc<SharedStorage>,
    uid: i64,
    plan_code: &str,
    days: i32,
) -> anyhow::Result<()> {
    let plan = match plan_code {
        "p" => "premium",
        "v" => "vip",
        _ => return Ok(()),
    };

    let days_opt = if days == 0 { None } else { Some(days) };
    shared_storage.update_user_plan_with_expiry(uid, plan, days_opt).await?;

    notify_user_plan_change(bot, uid, plan).await;
    show_user_detail(bot, chat_id, msg_id, shared_storage, uid).await?;
    Ok(())
}

async fn notify_user_plan_change(bot: &Bot, uid: i64, plan: &str) {
    let (emoji, name) = match plan {
        "premium" => ("⭐", "Premium"),
        "vip" => ("👑", "VIP"),
        _ => ("🆓", "Free"),
    };
    let text = format!(
        "💳 Your plan has been changed by administrator.\n\nNew plan: {} {}\n\nChanges take effect immediately! 🎉",
        emoji, name
    );
    if let Err(e) = bot.send_message(ChatId(uid), text).await {
        log::warn!("Failed to notify user {} about plan change: {}", uid, e);
    }
}

// --- Block toggle ---

async fn handle_toggle_block(
    bot: &Bot,
    chat_id: ChatId,
    msg_id: MessageId,
    shared_storage: &Arc<SharedStorage>,
    uid: i64,
    confirmed: bool,
) -> anyhow::Result<()> {
    if admin::is_admin(uid) {
        if let Err(e) = bot
            .edit_message_text(chat_id, msg_id, "❌ Cannot block an admin user")
            .reply_markup(InlineKeyboardMarkup::new(vec![vec![cb(
                "🔙 Back",
                format!("au:u:{}", uid),
            )]]))
            .await
        {
            log::warn!("Failed to edit block error: {}", e);
        }
        return Ok(());
    }

    let currently_blocked = shared_storage.is_user_blocked(uid).await?;

    if currently_blocked {
        shared_storage.set_user_blocked(uid, false).await?;
        show_user_detail(bot, chat_id, msg_id, shared_storage, uid).await?;
        return Ok(());
    }

    if !confirmed {
        let text = format!("⚠️ Block user {}?\n\nBlocked users cannot use the bot.", uid);
        let keyboard = InlineKeyboardMarkup::new(vec![
            vec![cb("🚫 Yes, block", format!("au:cb:{}", uid))],
            vec![cb("🔙 Cancel", format!("au:u:{}", uid))],
        ]);
        if let Err(e) = bot
            .edit_message_text(chat_id, msg_id, text)
            .reply_markup(keyboard)
            .await
        {
            log::warn!("Failed to edit block confirmation: {}", e);
        }
        return Ok(());
    }

    shared_storage.set_user_blocked(uid, true).await?;
    show_user_detail(bot, chat_id, msg_id, shared_storage, uid).await?;
    Ok(())
}

// --- Settings menu ---

const FORMATS: &[&str] = &["mp3", "mp4"];
const QUALITIES: &[&str] = &["best", "1080p", "720p", "480p", "360p"];
const BITRATES: &[&str] = &["128k", "192k", "256k", "320k"];
const LANGUAGES: &[&str] = &["ru", "en"];
const PROGRESS_STYLES: &[&str] = &["classic", "gradient", "emoji", "dots", "runner", "rpg", "fire", "moon"];

async fn show_settings_menu(
    bot: &Bot,
    chat_id: ChatId,
    msg_id: MessageId,
    shared_storage: &Arc<SharedStorage>,
    uid: i64,
) -> anyhow::Result<()> {
    let user = match shared_storage.get_user(uid).await? {
        Some(u) => u,
        None => return Ok(()),
    };

    let text = format!("⚙️ Settings: {}", username_display(&user));

    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![cb(
            format!("Format: {} ▸", user.download_format),
            format!("au:cs:{}:fmt:{}", uid, next_cycle(FORMATS, &user.download_format)),
        )],
        vec![cb(
            format!("Video: {} ▸", user.video_quality),
            format!("au:cs:{}:vid:{}", uid, next_cycle(QUALITIES, &user.video_quality)),
        )],
        vec![cb(
            format!("Audio: {} ▸", user.audio_bitrate),
            format!("au:cs:{}:aud:{}", uid, next_cycle(BITRATES, &user.audio_bitrate)),
        )],
        vec![cb(
            format!("Language: {} ▸", user.language),
            format!("au:cs:{}:lang:{}", uid, next_cycle(LANGUAGES, &user.language)),
        )],
        vec![cb(
            format!("Progress: {} ▸", user.progress_bar_style),
            format!(
                "au:cs:{}:prog:{}",
                uid,
                next_cycle(PROGRESS_STYLES, &user.progress_bar_style)
            ),
        )],
        vec![cb("🔙 Back", format!("au:u:{}", uid))],
    ]);

    if let Err(e) = bot
        .edit_message_text(chat_id, msg_id, text)
        .reply_markup(keyboard)
        .await
    {
        log::warn!("Failed to edit settings menu: {}", e);
    }
    Ok(())
}

fn next_cycle<'a>(options: &[&'a str], current: &str) -> &'a str {
    let idx = options.iter().position(|&o| o == current).unwrap_or(0);
    options[(idx + 1) % options.len()]
}

/// Validate that a setting value is in the allowed set.
fn validate_setting(key: &str, val: &str) -> bool {
    match key {
        "fmt" => FORMATS.contains(&val),
        "vid" => QUALITIES.contains(&val),
        "aud" => BITRATES.contains(&val),
        "lang" => LANGUAGES.contains(&val),
        "prog" => PROGRESS_STYLES.contains(&val),
        _ => false,
    }
}

// --- Change setting ---

async fn handle_change_setting(
    bot: &Bot,
    chat_id: ChatId,
    msg_id: MessageId,
    shared_storage: &Arc<SharedStorage>,
    uid: i64,
    key: &str,
    val: &str,
) -> anyhow::Result<()> {
    if !validate_setting(key, val) {
        log::warn!("Admin: invalid setting {}={} for user {}", key, val, uid);
        return Ok(());
    }

    match key {
        "fmt" => shared_storage.set_user_download_format(uid, val).await?,
        "vid" => shared_storage.set_user_video_quality(uid, val).await?,
        "aud" => shared_storage.set_user_audio_bitrate(uid, val).await?,
        "lang" => shared_storage.set_user_language(uid, val).await?,
        "prog" => shared_storage.set_user_progress_bar_style(uid, val).await?,
        _ => return Ok(()),
    }
    show_settings_menu(bot, chat_id, msg_id, shared_storage, uid).await?;
    Ok(())
}

// --- Admin search ---

pub async fn handle_admin_search(
    bot: &Bot,
    chat_id: ChatId,
    shared_storage: &Arc<SharedStorage>,
    query: &str,
) -> anyhow::Result<()> {
    clear_admin_searching(shared_storage, chat_id.0).await;

    let users = shared_storage.search_users(query).await?;

    if users.is_empty() {
        bot.send_message(chat_id, format!("🔍 No users found for: {}", query))
            .reply_markup(InlineKeyboardMarkup::new(vec![vec![cb("🔙 Back", "au:l:0:a")]]))
            .await?;
        return Ok(());
    }

    let mut text = format!("🔍 Search results for \"{}\":\n\n", query);
    let mut keyboard_rows: Vec<Vec<teloxide::types::InlineKeyboardButton>> = Vec::new();
    let mut row = Vec::new();

    for (i, user) in users.iter().enumerate() {
        let blocked_mark = if user.is_blocked { " 🚫" } else { "" };
        text.push_str(&format!(
            "{}. {} {}{} ({})\n",
            i + 1,
            user.plan.emoji(),
            username_display(user),
            blocked_mark,
            user.telegram_id
        ));

        let label = format!("{} {}", user.plan.emoji(), username_short(user));
        row.push(cb(label, format!("au:u:{}", user.telegram_id)));
        if row.len() == 2 {
            keyboard_rows.push(std::mem::take(&mut row));
        }
    }
    if !row.is_empty() {
        keyboard_rows.push(row);
    }
    keyboard_rows.push(vec![cb("🔙 Back", "au:l:0:a")]);

    let keyboard = InlineKeyboardMarkup::new(keyboard_rows);
    bot.send_message(chat_id, text).reply_markup(keyboard).await?;
    Ok(())
}

// --- Main callback dispatcher ---

pub async fn handle_callback(
    bot: &Bot,
    chat_id: ChatId,
    msg_id: MessageId,
    shared_storage: &Arc<SharedStorage>,
    data: &str,
) -> anyhow::Result<()> {
    let rest = match data.strip_prefix("au:") {
        Some(r) => r,
        None => return Ok(()),
    };

    let parts: Vec<&str> = rest.splitn(5, ':').collect();

    if !preserves_search_mode(parts.first().copied()) {
        clear_admin_searching(shared_storage, chat_id.0).await;
    }

    match parts.first().copied() {
        Some("l") => {
            let page = parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(0);
            let filter = parts.get(2).map(|f| Filter::from_code(f)).unwrap_or(Filter::All);
            show_user_list(bot, chat_id, Some(msg_id), shared_storage, page, filter).await?;
        }
        Some("u") => {
            if let Some(uid) = parts.get(1).and_then(|p| p.parse().ok()) {
                show_user_detail(bot, chat_id, msg_id, shared_storage, uid).await?;
            }
        }
        Some("sp") => {
            if let (Some(uid), Some(plan_code)) = (parts.get(1).and_then(|p| p.parse().ok()), parts.get(2)) {
                handle_set_plan(bot, chat_id, msg_id, shared_storage, uid, plan_code).await?;
            }
        }
        Some("se") => {
            if let (Some(uid), Some(plan_code), Some(days)) = (
                parts.get(1).and_then(|p| p.parse().ok()),
                parts.get(2).copied(),
                parts.get(3).and_then(|p| p.parse().ok()),
            ) {
                handle_set_expiry(bot, chat_id, msg_id, shared_storage, uid, plan_code, days).await?;
            }
        }
        Some("b") => {
            if let Some(uid) = parts.get(1).and_then(|p| p.parse().ok()) {
                handle_toggle_block(bot, chat_id, msg_id, shared_storage, uid, false).await?;
            }
        }
        Some("cb") => {
            if let Some(uid) = parts.get(1).and_then(|p| p.parse().ok()) {
                handle_toggle_block(bot, chat_id, msg_id, shared_storage, uid, true).await?;
            }
        }
        Some("st") => {
            if let Some(uid) = parts.get(1).and_then(|p| p.parse().ok()) {
                show_settings_menu(bot, chat_id, msg_id, shared_storage, uid).await?;
            }
        }
        Some("cs") => {
            if let (Some(uid), Some(key), Some(val)) = (
                parts.get(1).and_then(|p| p.parse().ok()),
                parts.get(2).copied(),
                parts.get(3).copied(),
            ) {
                handle_change_setting(bot, chat_id, msg_id, shared_storage, uid, key, val).await?;
            }
        }
        Some("s") => {
            set_admin_searching(shared_storage, chat_id.0).await;
            if let Err(e) = bot
                .edit_message_text(chat_id, msg_id, "🔍 Send a username or user ID to search:")
                .reply_markup(InlineKeyboardMarkup::new(vec![vec![cb("🔙 Cancel", "au:l:0:a")]]))
                .await
            {
                log::warn!("Failed to edit search prompt: {}", e);
            }
        }
        Some("noop") => {}
        _ => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::preserves_search_mode;

    #[test]
    fn search_mode_is_preserved_only_for_search_prompt_action() {
        assert!(preserves_search_mode(Some("s")));
        assert!(!preserves_search_mode(Some("l")));
        assert!(!preserves_search_mode(Some("u")));
        assert!(!preserves_search_mode(None));
    }
}
