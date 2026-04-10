use crate::core::validation::sanitize_filename;
use anyhow::Context;
use crate::storage::db::{self, DbPool, DownloadHistoryEntry};
use crate::telegram::admin::download_file_from_telegram;
use crate::telegram::Bot;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{CallbackQueryId, InlineKeyboardMarkup, InputFile, MessageId};

const ITEMS_PER_PAGE: usize = 5;

/// Maximum archive size for standard Bot API (50 MB).
const MAX_ARCHIVE_SIZE_STANDARD: i64 = 50 * 1024 * 1024;

/// Maximum archive size for local Bot API (2 GB, conservative).
const MAX_ARCHIVE_SIZE_LOCAL: i64 = 2 * 1024 * 1024 * 1024;

fn max_archive_size() -> i64 {
    let is_local = std::env::var("BOT_API_URL")
        .ok()
        .map(|u| !u.contains("api.telegram.org"))
        .unwrap_or(false);
    if is_local {
        MAX_ARCHIVE_SIZE_LOCAL
    } else {
        MAX_ARCHIVE_SIZE_STANDARD
    }
}

/// Re-export of the shared byte formatter under the local name so call
/// sites in this file don't need to change.
use doracore::core::format_bytes_i64 as format_file_size;

/// Main callback dispatcher for `arc:*` callbacks.
pub async fn handle_archive_callback(
    bot: &Bot,
    callback_id: CallbackQueryId,
    chat_id: ChatId,
    message_id: MessageId,
    data: &str,
    db_pool: Arc<DbPool>,
) -> Result<(), teloxide::RequestError> {
    let _ = bot.answer_callback_query(callback_id).await;

    let parts: Vec<&str> = data.splitn(4, ':').collect();
    let action = parts.get(1).copied().unwrap_or("");

    match action {
        "new" => handle_new(bot, chat_id, message_id, &db_pool).await,
        "tog" => {
            let download_id = parts.get(2).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
            handle_toggle(bot, chat_id, message_id, &db_pool, download_id).await
        }
        "pg" => {
            let page = parts.get(2).and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
            show_selection_page(bot, chat_id, message_id, &db_pool, page).await
        }
        "all" => {
            let page = parts.get(2).and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
            handle_select_page(bot, chat_id, message_id, &db_pool, page, true).await
        }
        "none" => {
            let page = parts.get(2).and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
            handle_select_page(bot, chat_id, message_id, &db_pool, page, false).await
        }
        "preset" => {
            let preset = parts.get(2).copied().unwrap_or("");
            handle_preset(bot, chat_id, message_id, &db_pool, preset).await
        }
        "clear" => handle_clear(bot, chat_id, message_id, &db_pool).await,
        "build" => handle_build(bot, chat_id, message_id, db_pool).await,
        "cancel" => handle_cancel(bot, chat_id, message_id, &db_pool).await,
        _ => Ok(()),
    }
}

/// Start new archive session and show selection page 0.
async fn handle_new(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: &DbPool,
) -> Result<(), teloxide::RequestError> {
    {
        let conn = match db::get_connection(db_pool) {
            Ok(c) => c,
            Err(e) => {
                log::error!("Archive: DB connection error: {}", e);
                return Ok(());
            }
        };

        let downloads = db::get_download_history_filtered(&conn, chat_id.0, None, None, None).unwrap_or_default();
        if downloads.is_empty() {
            bot.edit_message_text(chat_id, message_id, "📦 No downloads to archive.")
                .await?;
            return Ok(());
        }

        if let Err(e) = db::create_archive_session(&conn, chat_id.0) {
            log::error!("Archive: failed to create session: {}", e);
            return Ok(());
        }
    } // conn dropped here

    show_selection_page(bot, chat_id, message_id, db_pool, 0).await
}

/// Toggle a single item and refresh the page.
async fn handle_toggle(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: &DbPool,
    download_id: i64,
) -> Result<(), teloxide::RequestError> {
    let page = {
        let conn = match db::get_connection(db_pool) {
            Ok(c) => c,
            Err(e) => {
                log::error!("Archive: DB error: {}", e);
                return Ok(());
            }
        };

        let session = match db::get_active_archive_session(&conn, chat_id.0) {
            Ok(Some(s)) => s,
            _ => {
                bot.edit_message_text(chat_id, message_id, "📦 Session expired. Start over from /downloads.")
                    .await?;
                return Ok(());
            }
        };

        let _ = db::toggle_archive_item(&conn, &session.id, download_id);

        let downloads = db::get_download_history_filtered(&conn, chat_id.0, None, None, None).unwrap_or_default();
        downloads
            .iter()
            .position(|d| d.id == download_id)
            .map(|pos| pos / ITEMS_PER_PAGE)
            .unwrap_or(0)
    }; // conn dropped

    show_selection_page(bot, chat_id, message_id, db_pool, page).await
}

/// Select or deselect all items on a specific page.
async fn handle_select_page(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: &DbPool,
    page: usize,
    select: bool,
) -> Result<(), teloxide::RequestError> {
    {
        let conn = match db::get_connection(db_pool) {
            Ok(c) => c,
            Err(e) => {
                log::error!("Archive: DB error: {}", e);
                return Ok(());
            }
        };

        let session = match db::get_active_archive_session(&conn, chat_id.0) {
            Ok(Some(s)) => s,
            _ => {
                bot.edit_message_text(chat_id, message_id, "📦 Session expired. Start over from /downloads.")
                    .await?;
                return Ok(());
            }
        };

        let downloads = db::get_download_history_filtered(&conn, chat_id.0, None, None, None).unwrap_or_default();
        let start = page * ITEMS_PER_PAGE;
        let page_items: Vec<i64> = downloads
            .iter()
            .skip(start)
            .take(ITEMS_PER_PAGE)
            .map(|d| d.id)
            .collect();

        if select {
            let _ = db::add_archive_items_bulk(&conn, &session.id, &page_items);
        } else {
            let selected = db::get_archive_item_ids(&conn, &session.id).unwrap_or_default();
            for id in &page_items {
                if selected.contains(id) {
                    let _ = db::toggle_archive_item(&conn, &session.id, *id);
                }
            }
        }
    } // conn dropped

    show_selection_page(bot, chat_id, message_id, db_pool, page).await
}

/// Handle preset selections (today, last10).
async fn handle_preset(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: &DbPool,
    preset: &str,
) -> Result<(), teloxide::RequestError> {
    {
        let conn = match db::get_connection(db_pool) {
            Ok(c) => c,
            Err(e) => {
                log::error!("Archive: DB error: {}", e);
                return Ok(());
            }
        };

        let session = match db::get_active_archive_session(&conn, chat_id.0) {
            Ok(Some(s)) => s,
            _ => {
                bot.edit_message_text(chat_id, message_id, "📦 Session expired. Start over from /downloads.")
                    .await?;
                return Ok(());
            }
        };

        let downloads = db::get_download_history_filtered(&conn, chat_id.0, None, None, None).unwrap_or_default();
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();

        let ids: Vec<i64> = match preset {
            "today" => downloads
                .iter()
                .filter(|d| d.downloaded_at.starts_with(&today))
                .map(|d| d.id)
                .collect(),
            "last10" => downloads.iter().take(10).map(|d| d.id).collect(),
            _ => vec![],
        };

        let _ = db::clear_archive_items(&conn, &session.id);
        if !ids.is_empty() {
            let _ = db::add_archive_items_bulk(&conn, &session.id, &ids);
        }
    } // conn dropped

    show_selection_page(bot, chat_id, message_id, db_pool, 0).await
}

/// Clear all selections.
async fn handle_clear(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: &DbPool,
) -> Result<(), teloxide::RequestError> {
    {
        let conn = match db::get_connection(db_pool) {
            Ok(c) => c,
            Err(e) => {
                log::error!("Archive: DB error: {}", e);
                return Ok(());
            }
        };

        let session = match db::get_active_archive_session(&conn, chat_id.0) {
            Ok(Some(s)) => s,
            _ => {
                bot.edit_message_text(chat_id, message_id, "📦 Session expired. Start over from /downloads.")
                    .await?;
                return Ok(());
            }
        };

        let _ = db::clear_archive_items(&conn, &session.id);
    } // conn dropped

    show_selection_page(bot, chat_id, message_id, db_pool, 0).await
}

/// Cancel session.
async fn handle_cancel(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: &DbPool,
) -> Result<(), teloxide::RequestError> {
    {
        let conn = match db::get_connection(db_pool) {
            Ok(c) => c,
            Err(e) => {
                log::error!("Archive: DB error: {}", e);
                return Ok(());
            }
        };

        if let Ok(Some(session)) = db::get_active_archive_session(&conn, chat_id.0) {
            let _ = db::delete_archive_session(&conn, &session.id);
        }
    }

    bot.edit_message_text(chat_id, message_id, "📦 Archive cancelled.")
        .await?;
    Ok(())
}

/// Build the ZIP and send it.
async fn handle_build(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: Arc<DbPool>,
) -> Result<(), teloxide::RequestError> {
    // Gather all DB data upfront, then drop connection
    let (session_id, items) = {
        let conn = match db::get_connection(&db_pool) {
            Ok(c) => c,
            Err(e) => {
                log::error!("Archive: DB error: {}", e);
                return Ok(());
            }
        };

        let session = match db::get_active_archive_session(&conn, chat_id.0) {
            Ok(Some(s)) => s,
            _ => {
                bot.edit_message_text(chat_id, message_id, "📦 Session expired. Start over from /downloads.")
                    .await?;
                return Ok(());
            }
        };

        let items = db::get_archive_items_full(&conn, &session.id).unwrap_or_default();
        if items.is_empty() {
            bot.edit_message_text(chat_id, message_id, "📦 No files selected. Select at least one file.")
                .await?;
            return Ok(());
        }

        let (count, total_size) = db::count_archive_items(&conn, &session.id).unwrap_or((0, 0));
        let size_limit = max_archive_size();

        if total_size > size_limit {
            let msg = format!(
                "📦 Archive too large: {} ({} files)\nMax: {}",
                format_file_size(total_size),
                count,
                format_file_size(size_limit),
            );
            bot.edit_message_text(chat_id, message_id, msg).await?;
            return Ok(());
        }

        let _ = db::update_archive_session_status(&conn, &session.id, "building");

        (session.id, items)
    }; // conn dropped

    // Progress message
    bot.edit_message_text(
        chat_id,
        message_id,
        format!("📦 Building archive (0/{})...", items.len()),
    )
    .await?;

    // Create temp directory
    let archive_dir = PathBuf::from(crate::core::config::TEMP_FILES_DIR.as_str())
        .join("doradura_archive")
        .join(&session_id);
    let guard = match crate::core::utils::TempDirGuard::from_path(archive_dir).await {
        Ok(g) => g,
        Err(e) => {
            log::error!("Archive: failed to create temp dir: {}", e);
            bot.edit_message_text(chat_id, message_id, "❌ Failed to create archive.")
                .await?;
            update_session_status(&db_pool, &session_id, "failed");
            return Ok(());
        }
    };
    let temp_dir = guard.path().to_path_buf();

    // Download files from Telegram
    let mut downloaded_files: Vec<(String, PathBuf)> = Vec::new();
    let mut skipped = 0usize;
    let mut name_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for (i, item) in items.iter().enumerate() {
        let file_id = match &item.file_id {
            Some(fid) if !fid.is_empty() => fid,
            _ => {
                skipped += 1;
                continue;
            }
        };

        // Build filename
        let base_name = if let Some(ref author) = item.author {
            if author.is_empty() {
                item.title.clone()
            } else {
                format!("{} - {}", author, item.title)
            }
        } else {
            item.title.clone()
        };

        let sanitized = sanitize_filename(&base_name);
        let ext = &item.format;

        // Deduplicate names
        let key = format!("{}.{}", sanitized, ext);
        let count = name_counts.entry(key).or_insert(0);
        *count += 1;
        let filename = if *count > 1 {
            format!("{} ({}).{}", sanitized, count, ext)
        } else {
            format!("{}.{}", sanitized, ext)
        };

        let dest_path = temp_dir.join(&filename);

        match download_file_from_telegram(bot, file_id, Some(dest_path.clone())).await {
            Ok(_) => {
                downloaded_files.push((filename, dest_path));
            }
            Err(e) => {
                log::warn!("Archive: failed to download file_id {}: {}", file_id, e);
                skipped += 1;
            }
        }

        // Update progress every 2 files
        if (i + 1) % 2 == 0 || i + 1 == items.len() {
            let _ = bot
                .edit_message_text(
                    chat_id,
                    message_id,
                    format!("📦 Downloading files ({}/{})...", i + 1, items.len()),
                )
                .await;
        }
    }

    if downloaded_files.is_empty() {
        bot.edit_message_text(
            chat_id,
            message_id,
            "❌ All file downloads failed. Cannot create archive.",
        )
        .await?;
        update_session_status(&db_pool, &session_id, "failed");
        return Ok(());
    }

    // Create ZIP
    let _ = bot
        .edit_message_text(chat_id, message_id, "📦 Creating ZIP archive...")
        .await;

    let date_str = chrono::Utc::now().format("%Y%m%d").to_string();
    let zip_filename = format!("doradura_{}.zip", date_str);
    let zip_path = temp_dir.join(&zip_filename);

    let files_for_zip = downloaded_files.clone();
    let zip_path_clone = zip_path.clone();
    let zip_result = tokio::task::spawn_blocking(move || create_zip_file(&zip_path_clone, &files_for_zip)).await;

    match zip_result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            log::error!("Archive: ZIP creation failed: {}", e);

            bot.edit_message_text(chat_id, message_id, "❌ Failed to create ZIP archive.")
                .await?;
            update_session_status(&db_pool, &session_id, "failed");
            return Ok(());
        }
        Err(e) => {
            log::error!("Archive: ZIP task panicked: {}", e);

            bot.edit_message_text(chat_id, message_id, "❌ Failed to create ZIP archive.")
                .await?;
            update_session_status(&db_pool, &session_id, "failed");
            return Ok(());
        }
    }

    // Send ZIP
    let _ = bot
        .edit_message_text(chat_id, message_id, "📦 Sending archive...")
        .await;

    let zip_file_size = tokio::fs::metadata(&zip_path).await.map(|m| m.len()).unwrap_or(0);
    let caption = format!(
        "📦 Archive: {} files ({})",
        downloaded_files.len(),
        format_file_size(zip_file_size as i64),
    );
    let caption = if skipped > 0 {
        format!("{}\n⚠️ {} files skipped (download failed)", caption, skipped)
    } else {
        caption
    };

    match bot
        .send_document(chat_id, InputFile::file(&zip_path).file_name(zip_filename))
        .caption(caption)
        .await
    {
        Ok(_) => {
            update_session_status(&db_pool, &session_id, "done");
        }
        Err(e) => {
            log::error!("Archive: failed to send ZIP: {}", e);
            bot.edit_message_text(
                chat_id,
                message_id,
                format!(
                    "❌ Failed to send archive ({}). File may be too large.",
                    format_file_size(zip_file_size as i64)
                ),
            )
            .await?;
            update_session_status(&db_pool, &session_id, "failed");
        }
    }

    // Cleanup

    let _ = bot.delete_message(chat_id, message_id).await;

    Ok(())
}

/// Helper to update session status without holding connection across await.
fn update_session_status(db_pool: &DbPool, session_id: &str, status: &str) {
    if let Ok(conn) = db::get_connection(db_pool) {
        let _ = db::update_archive_session_status(&conn, session_id, status);
    }
}

/// Render the selection page UI and edit the message.
/// All DB work is done before the async Telegram call to avoid Send issues.
async fn show_selection_page(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    db_pool: &DbPool,
    page: usize,
) -> Result<(), teloxide::RequestError> {
    // Gather all data synchronously, drop conn before any await
    let page_data = gather_selection_data(db_pool, chat_id.0);

    let page_data = match page_data {
        Some(data) => data,
        None => {
            return expired_message(bot, chat_id, message_id).await;
        }
    };

    let total_pages = page_data.downloads.len().div_ceil(ITEMS_PER_PAGE);
    let page = if total_pages == 0 { 0 } else { page.min(total_pages - 1) };

    let start = page * ITEMS_PER_PAGE;
    let page_items: Vec<&DownloadHistoryEntry> = page_data.downloads.iter().skip(start).take(ITEMS_PER_PAGE).collect();

    // Build text
    let mut text = String::from("📦 Select files for archive\n\n");
    for item in &page_items {
        let checked = if page_data.selected_ids.contains(&item.id) {
            "☑️"
        } else {
            "☐"
        };
        let title = if item.title.chars().count() > 30 {
            let t: String = item.title.chars().take(27).collect();
            format!("{}...", t)
        } else {
            item.title.clone()
        };
        let author_prefix = item
            .author
            .as_deref()
            .filter(|a| !a.is_empty())
            .map(|a| {
                if a.chars().count() > 15 {
                    let t: String = a.chars().take(13).collect();
                    format!("{}... - ", t)
                } else {
                    format!("{} - ", a)
                }
            })
            .unwrap_or_default();
        let size = item
            .file_size
            .map(|s| format!(", {}", format_file_size(s)))
            .unwrap_or_default();
        text.push_str(&format!(
            "{} {}{} ({}{})\n",
            checked, author_prefix, title, item.format, size
        ));
    }

    text.push_str(&format!(
        "\nSelected: {} files ({})",
        page_data.sel_count,
        format_file_size(page_data.sel_size),
    ));

    // Build keyboard
    let mut keyboard_rows = Vec::new();

    // Toggle buttons row
    let mut toggle_row = Vec::new();
    for (i, item) in page_items.iter().enumerate() {
        let checked = page_data.selected_ids.contains(&item.id);
        let label = if checked {
            format!("☑️ {}", i + 1)
        } else {
            format!("☐ {}", i + 1)
        };
        toggle_row.push(crate::telegram::cb(label, format!("arc:tog:{}", item.id)));
    }
    if !toggle_row.is_empty() {
        keyboard_rows.push(toggle_row);
    }

    // Navigation row
    if total_pages > 1 {
        let mut nav_row = Vec::new();
        if page > 0 {
            nav_row.push(crate::telegram::cb("⬅️", format!("arc:pg:{}", page - 1)));
        }
        nav_row.push(crate::telegram::cb(
            format!("{}/{}", page + 1, total_pages),
            format!("arc:pg:{}", page),
        ));
        if page < total_pages - 1 {
            nav_row.push(crate::telegram::cb("➡️", format!("arc:pg:{}", page + 1)));
        }
        keyboard_rows.push(nav_row);
    }

    // Select all / none
    keyboard_rows.push(vec![
        crate::telegram::cb("✅ All", format!("arc:all:{}", page)),
        crate::telegram::cb("⬜ None", format!("arc:none:{}", page)),
    ]);

    // Presets
    keyboard_rows.push(vec![
        crate::telegram::cb("⚡ Today", "arc:preset:today"),
        crate::telegram::cb("🔢 Last 10", "arc:preset:last10"),
    ]);

    // Build / Cancel
    let build_label = if page_data.sel_count > 0 {
        format!(
            "📦 Build ({}, {})",
            page_data.sel_count,
            format_file_size(page_data.sel_size)
        )
    } else {
        "📦 Build".to_string()
    };
    keyboard_rows.push(vec![
        crate::telegram::cb(build_label, "arc:build"),
        crate::telegram::cb("❌ Cancel", "arc:cancel"),
    ]);

    let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

    bot.edit_message_text(chat_id, message_id, text)
        .reply_markup(keyboard)
        .await?;

    Ok(())
}

async fn expired_message(bot: &Bot, chat_id: ChatId, message_id: MessageId) -> Result<(), teloxide::RequestError> {
    bot.edit_message_text(chat_id, message_id, "📦 Session expired. Start over from /downloads.")
        .await?;
    Ok(())
}

/// Gather all DB data needed for the selection page. Returns None if session expired/missing.
fn gather_selection_data(db_pool: &DbPool, user_id: i64) -> Option<SelectionPageData> {
    let conn = db::get_connection(db_pool).ok()?;
    let session = db::get_active_archive_session(&conn, user_id).ok().flatten()?;
    let downloads = db::get_download_history_filtered(&conn, user_id, None, None, None).unwrap_or_default();
    let selected_ids = db::get_archive_item_ids(&conn, &session.id).unwrap_or_default();
    let (sel_count, sel_size) = db::count_archive_items(&conn, &session.id).unwrap_or((0, 0));
    Some(SelectionPageData {
        downloads,
        selected_ids,
        sel_count,
        sel_size,
    })
}

struct SelectionPageData {
    downloads: Vec<DownloadHistoryEntry>,
    selected_ids: HashSet<i64>,
    sel_count: i64,
    sel_size: i64,
}

/// Creates a ZIP file at `zip_path` from the given files (sync, run in spawn_blocking).
fn create_zip_file(zip_path: &std::path::Path, files: &[(String, PathBuf)]) -> anyhow::Result<()> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    let file = std::fs::File::create(zip_path).with_context(|| "create zip")?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    for (name, path) in files {
        zip.start_file(name, options)
            .map_err(|e| anyhow::anyhow!("start file '{}': {}", name, e))?;
        let data = std::fs::read(path).map_err(|e| anyhow::anyhow!("read '{}': {}", name, e))?;
        zip.write_all(&data).map_err(|e| anyhow::anyhow!("write '{}': {}", name, e))?;
    }

    zip.finish().with_context(|| "finish zip")?;
    Ok(())
}

