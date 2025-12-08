//! Telegram bot integration and handlers

pub mod cache;
pub mod commands;
pub mod menu;
pub mod notifications;
pub mod preview;
pub mod types;
pub mod webapp;
pub mod webapp_auth;

// Re-exports for convenience
pub use commands::handle_message;
pub use menu::{handle_menu_callback, show_enhanced_main_menu, show_main_menu};
pub use webapp::{create_webapp_router, run_webapp_server, WebAppAction, WebAppData};
