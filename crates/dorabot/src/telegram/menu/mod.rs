pub mod admin_users;
mod audio_effects;
mod callback_admin;
mod callback_kind;
mod callback_router;
mod callback_settings;
pub(crate) mod helpers;
pub(crate) mod lyrics;
mod main_menu;
pub mod player;
pub mod playlist;
pub mod playlist_integrations;
pub mod ringtone;
pub mod search;
mod services;
mod settings;
pub mod vault;
pub mod vlipsy;

// Re-export all previously-public functions
pub use callback_router::handle_menu_callback;
pub use main_menu::{send_main_menu_as_new, show_enhanced_main_menu, show_main_menu};
pub use services::show_services_menu;
pub use settings::{
    send_download_type_menu_as_new, show_audio_bitrate_menu, show_download_type_menu, show_language_menu,
    show_language_selection_menu, show_progress_bar_style_menu, show_video_quality_menu,
};
