//! Telegram bot integration and handlers

pub mod commands;
pub mod menu;
pub mod notifications;
pub mod preview;
pub mod webapp;
pub mod webapp_auth;

// Re-exports for convenience
pub use commands::handle_message;
pub use menu::{handle_menu_callback, show_main_menu};
pub use webapp::{create_webapp_router, run_webapp_server, WebAppAction, WebAppData};
