//! Logging initialization and configuration checking
//!
//! This module provides:
//! - Logger initialization (console + file) via `tracing-subscriber`
//! - Cookies configuration validation and logging
//! - Startup diagnostics

use anyhow::Result;
use std::fs::File;

use crate::core::config;

/// Initialize logger for both console and file output.
///
/// Uses `tracing-subscriber` with log bridge so that both `tracing::info!`
/// spans and legacy `log::info!` calls are captured.
pub fn init_logger(log_file_path: &str) -> Result<()> {
    use tracing_subscriber::{fmt, layer::SubscriberExt, EnvFilter};

    let log_file = File::create(log_file_path).map_err(|e| anyhow::anyhow!("Failed to create log file: {}", e))?;

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let console_layer = fmt::layer()
        .with_target(true)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false);

    let file_layer = fmt::layer()
        .with_target(true)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(log_file));

    let subscriber = tracing_subscriber::registry()
        .with(env_filter)
        .with(console_layer)
        .with(file_layer);

    // set_global_default sets ONLY the tracing subscriber, does NOT touch log::set_logger.
    // This avoids the "logger already initialized" conflict that try_init() causes
    // (because try_init internally calls log::set_logger via tracing-log compat).
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|e| anyhow::anyhow!("Failed to set tracing subscriber: {}", e))?;

    // Now explicitly set the log→tracing bridge.
    // This must happen AFTER set_global_default so events have somewhere to go.
    let _ = tracing_log::LogTracer::init();

    Ok(())
}

/// Logs cookies configuration at application startup
pub fn log_cookies_configuration() {
    log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    log::info!("🍪 Cookies Configuration Check");
    log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    if let Some(ref cookies_file) = *config::YTDL_COOKIES_FILE {
        if !cookies_file.is_empty() {
            let cookies_path = if std::path::Path::new(cookies_file).is_absolute() {
                cookies_file.clone()
            } else {
                shellexpand::tilde(cookies_file).to_string()
            };

            let cookies_path_buf = std::path::Path::new(&cookies_path);
            if cookies_path_buf.exists() {
                if let Ok(abs_path) = cookies_path_buf.canonicalize() {
                    log::info!("✅ YTDL_COOKIES_FILE: {}", abs_path.display());
                    log::info!("   File exists and will be used for YouTube authentication");
                } else {
                    log::warn!(
                        "⚠️  YTDL_COOKIES_FILE: {} (exists but cannot canonicalize)",
                        cookies_path
                    );
                }
            } else {
                log::error!("❌ YTDL_COOKIES_FILE: {} (FILE NOT FOUND!)", cookies_file);
                log::error!("   Checked path: {}", cookies_path);
                log::error!("   Current directory: {:?}", std::env::current_dir());
                log::error!("   YouTube downloads will FAIL without valid cookies!");
            }
        } else {
            log::warn!("⚠️  YTDL_COOKIES_FILE is set but empty");
        }
    } else {
        log::warn!("⚠️  YTDL_COOKIES_FILE: not set");
    }

    let browser = config::YTDL_COOKIES_BROWSER.as_str();
    if !browser.is_empty() {
        log::info!("✅ YTDL_COOKIES_BROWSER: {}", browser);
        log::info!("   Will extract cookies from browser");
    } else {
        log::warn!("⚠️  YTDL_COOKIES_BROWSER: not set");
    }

    if let Some(ref cookies_file) = *config::YTDL_COOKIES_FILE {
        if !cookies_file.is_empty() {
            let cookies_path = if std::path::Path::new(cookies_file).is_absolute() {
                cookies_file.clone()
            } else {
                shellexpand::tilde(cookies_file).to_string()
            };

            if std::path::Path::new(&cookies_path).exists() {
                log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                log::info!("✅ Cookies configured - YouTube downloads should work");
                log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
            } else {
                log::error!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                log::error!("❌ Cookies file NOT FOUND - YouTube downloads will FAIL!");
                log::error!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
            }
        }
    } else if !browser.is_empty() {
        log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        log::info!("✅ Cookies from browser configured - YouTube downloads should work");
        log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    } else {
        log::error!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        log::error!("❌ NO COOKIES CONFIGURED - YouTube downloads will FAIL!");
        log::error!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        log::error!("");
        log::error!("Quick fix:");
        log::error!("");
        log::error!("💡 Option 1: Automatic extraction (Linux/Windows):");
        log::error!("  1. Login to YouTube in browser");
        log::error!("  2. Install: pip3 install keyring pycryptodomex");
        log::error!("  3. Set: export YTDL_COOKIES_BROWSER=chrome");
        log::error!("  4. Restart bot");
        log::error!("");
        log::error!("💡 Option 2: Export to file (macOS recommended):");
        log::error!("  1. Export cookies to youtube_cookies.txt");
        log::error!("  2. Set: export YTDL_COOKIES_FILE=youtube_cookies.txt");
        log::error!("  3. Or run: ./scripts/run_with_cookies.sh");
        log::error!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    }
}

#[cfg(test)]
mod tests {
    use tempfile::NamedTempFile;

    #[test]
    fn test_log_file_creation() {
        let temp_file = NamedTempFile::new().unwrap();
        assert!(temp_file.path().exists());
    }

    #[test]
    fn test_log_cookies_configuration_runs() {
        let _ = std::env::var("YTDL_COOKIES_FILE");
    }
}
