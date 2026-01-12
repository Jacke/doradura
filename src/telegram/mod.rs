//! Telegram bot integration and handlers

pub mod admin;
pub mod analytics;
pub mod bot_api_logger;
pub mod bot;
pub mod cache;
pub mod commands;
pub mod cuts;
pub mod downloads;
pub mod feedback;
pub mod markdown;
pub mod menu;
pub mod notifications;
pub mod preview;
pub mod reactions;
pub mod types;
pub mod voice;
pub mod webapp;
pub mod webapp_auth;

// Re-exports for convenience
pub use admin::{
    download_file_from_telegram, escape_markdown, handle_admin_command, handle_backup_command,
    handle_botapi_speed_command, handle_charges_command, handle_cookies_file_upload, handle_download_tg_command,
    handle_downsub_health_command, handle_sent_files_command, handle_setplan_command, handle_transactions_command,
    handle_update_cookies_command, handle_users_command, is_admin, notify_admin_cookies_refresh,
};
pub use analytics::{handle_analytics_command, handle_health_command, handle_metrics_command, handle_revenue_command};
pub use bot_api_logger::Bot;
pub use bot::{create_bot, is_message_addressed_to_bot, setup_all_language_commands, setup_chat_bot_commands, Command};
pub use commands::{handle_downsub_command, handle_info_command, handle_message};
pub use markdown::send_message_markdown_v2;
pub use menu::{handle_menu_callback, show_enhanced_main_menu, show_language_selection_menu, show_main_menu};
pub use reactions::try_set_reaction;
pub use voice::{send_random_voice_message, send_voice_with_waveform, VOICE_FILES};
pub use webapp::{create_webapp_router, run_webapp_server, WebAppAction, WebAppData};
