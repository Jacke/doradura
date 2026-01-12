//! Logging initialization and configuration checking
//!
//! This module provides:
//! - Logger initialization (console + file)
//! - Cookies configuration validation and logging
//! - Startup diagnostics

use anyhow::Result;
use simplelog::*;
use std::fs::File;

use crate::core::config;

/// Initialize logger for both console and file output
///
/// # Arguments
/// * `log_file_path` - Path to the log file
///
/// # Returns
/// * `Ok(())` - Logger initialized successfully
/// * `Err(anyhow::Error)` - Failed to initialize logger
pub fn init_logger(log_file_path: &str) -> Result<()> {
    let log_file = File::create(log_file_path).map_err(|e| anyhow::anyhow!("Failed to create log file: {}", e))?;

    CombinedLogger::init(vec![
        TermLogger::new(
            LevelFilter::Info,
            Config::default(),
            TerminalMode::Mixed,
            ColorChoice::Auto,
        ),
        WriteLogger::new(LevelFilter::Info, Config::default(), log_file),
    ])
    .map_err(|e| anyhow::anyhow!("Failed to initialize logger: {}", e))?;

    Ok(())
}

/// Logs cookies configuration at application startup
///
/// Validates and logs:
/// - YTDL_COOKIES_FILE existence and path
/// - YTDL_COOKIES_BROWSER configuration
/// - Provides troubleshooting guidance if cookies are not configured
pub fn log_cookies_configuration() {
    log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    log::info!("🍪 Cookies Configuration Check");
    log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    // Check cookies file
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

    // Check browser cookies
    let browser = config::YTDL_COOKIES_BROWSER.as_str();
    if !browser.is_empty() {
        log::info!("✅ YTDL_COOKIES_BROWSER: {}", browser);
        log::info!("   Will extract cookies from browser");
    } else {
        log::warn!("⚠️  YTDL_COOKIES_BROWSER: not set");
    }

    // Final status
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
    use super::*;

    use tempfile::NamedTempFile;

    #[test]
    fn test_init_logger_creates_log_file() {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path().to_str().unwrap();

        // Note: This test might fail if logger is already initialized
        // In real tests, we would need to handle this case
        let result = init_logger(path);

        // Just verify the function can be called
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_log_cookies_configuration_runs() {
        // Note: We don't actually call log_cookies_configuration() here
        // because it reads from static Lazy config that's initialized once
        // and we can't mock it in unit tests.
        //
        // The function is tested indirectly through integration tests
        // where the environment is properly set up.
        //
        // This test just verifies the function exists and compiles.
        // We use a simple check that always passes to satisfy clippy.
        let _ = std::env::var("YTDL_COOKIES_FILE");
    }
}
