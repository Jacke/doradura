//! Telegram bot integration and handlers

use teloxide::types::InlineKeyboardButton;

/// Max callback data size allowed by Telegram Bot API (bytes).
const CALLBACK_DATA_MAX: usize = 64;

/// Create an inline keyboard callback button with data length validation.
///
/// In debug/test builds, panics if `data` exceeds 64 bytes (Telegram's limit).
/// In release builds, truncates to 64 bytes and logs a warning.
///
/// Use this instead of `InlineKeyboardButton::callback` everywhere to catch
/// `BUTTON_DATA_INVALID` errors before they reach Telegram.
pub fn cb(label: impl Into<String>, data: impl Into<String>) -> InlineKeyboardButton {
    let data = data.into();
    if data.len() > CALLBACK_DATA_MAX {
        debug_assert!(
            false,
            "callback data too long ({} bytes, max {}): {}",
            data.len(),
            CALLBACK_DATA_MAX,
            data
        );
        log::error!(
            "callback data too long ({} bytes, max {}): {}",
            data.len(),
            CALLBACK_DATA_MAX,
            &data[..data.len().min(100)]
        );
        // Truncate at a clean UTF-8 boundary to avoid partial characters
        let truncated: String = data
            .chars()
            .take_while({
                let mut len = 0;
                move |c| {
                    len += c.len_utf8();
                    len <= CALLBACK_DATA_MAX
                }
            })
            .collect();
        return InlineKeyboardButton::callback(label, truncated);
    }
    InlineKeyboardButton::callback(label, data)
}

pub mod admin;
pub mod analytics;
pub mod bot;
pub mod bot_api_logger;
pub mod cache;
pub mod commands;
pub mod cuts;
pub mod downloads;
pub mod feedback;
pub mod handlers;
pub mod instagram;
pub mod markdown;
pub mod menu;
pub mod notifications;
pub mod operation;
pub mod preview;
pub mod reactions;
pub mod subscriptions;
pub mod types;
pub mod videos;
pub mod voice;

// Re-exports for convenience
pub use admin::{
    download_file_from_telegram, download_file_with_fallback, escape_markdown, handle_admin_command,
    handle_backup_command, handle_botapi_speed_command, handle_broadcast_command, handle_browser_callback,
    handle_browser_login_command, handle_browser_status_command, handle_charges_command,
    handle_check_ytdlp_version_callback, handle_cookies_file_upload, handle_download_tg_command,
    handle_downsub_health_command, handle_ig_cookies_file_upload, handle_send_command, handle_sent_files_command,
    handle_setplan_command, handle_transactions_command, handle_update_cookies_command,
    handle_update_ig_cookies_command, handle_update_ytdlp_callback, handle_update_ytdlp_command, handle_users_command,
    handle_version_command, is_admin, notify_admin_cookies_refresh,
};
pub use analytics::{handle_analytics_command, handle_health_command, handle_metrics_command, handle_revenue_command};
pub use bot::{create_bot, is_message_addressed_to_bot, setup_all_language_commands, setup_chat_bot_commands, Command};
pub use bot_api_logger::Bot;
pub use commands::{handle_downsub_command, handle_info_command, handle_message};
pub use handlers::{schema, HandlerDeps, HandlerError};
pub use markdown::send_message_markdown_v2;
pub use menu::{
    handle_menu_callback, show_enhanced_main_menu, show_language_selection_menu, show_main_menu, show_services_menu,
};
pub use operation::{
    Completed, InProgress, MarkdownV2Formatter, MessageFormatter, NotStarted, Operation, OperationBuilder,
    OperationError, OperationInfo, OperationStatus, PlainTextFormatter, DEFAULT_EMOJI,
};
pub use reactions::{emoji, success_reaction_for_format, try_set_reaction};
pub use videos::{handle_videos_callback, show_videos_page};
pub use voice::{send_random_voice_message, send_voice_with_waveform, VOICE_FILES};
