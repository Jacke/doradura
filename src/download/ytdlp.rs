use crate::core::config;
use crate::core::error::AppError;
use crate::download::error::DownloadError;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::process::Command as TokioCommand;
use tokio::time::{timeout, Duration};

/// Auto-update interval for yt-dlp (1 hour)
const AUTO_UPDATE_INTERVAL_HOURS: u64 = 1;

/// URL for downloading yt-dlp nightly builds
const NIGHTLY_URL: &str = "https://github.com/yt-dlp/yt-dlp-nightly-builds/releases/latest/download/yt-dlp";

/// Flag to stop the background auto-update task
static STOP_AUTO_UPDATE: AtomicBool = AtomicBool::new(false);

/// Returns the current yt-dlp version.
pub fn get_current_version() -> String {
    let ytdl_bin = &*config::YTDL_BIN;
    Command::new(ytdl_bin)
        .arg("--version")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Downloads and installs the latest yt-dlp nightly build.
async fn download_nightly_ytdlp() -> Result<(String, String), AppError> {
    let ytdl_bin = &*config::YTDL_BIN;
    let old_version = get_current_version();

    log::info!("Downloading yt-dlp nightly build...");

    // Download via wget
    let download_result = timeout(
        Duration::from_secs(120),
        TokioCommand::new("wget")
            .args(["-q", "-O", ytdl_bin, NIGHTLY_URL])
            .output(),
    )
    .await;

    match download_result {
        Ok(Ok(output)) => {
            if !output.status.success() {
                // Try curl as fallback
                log::info!("wget failed, trying curl...");

                let curl_result = timeout(
                    Duration::from_secs(120),
                    TokioCommand::new("curl")
                        .args(["-fsSL", "-o", ytdl_bin, NIGHTLY_URL])
                        .output(),
                )
                .await;

                match curl_result {
                    Ok(Ok(curl_output)) => {
                        if !curl_output.status.success() {
                            let stderr = String::from_utf8_lossy(&curl_output.stderr);
                            return Err(AppError::Download(DownloadError::YtDlp(format!(
                                "Failed to download yt-dlp nightly: {}",
                                stderr
                            ))));
                        }
                    }
                    Ok(Err(e)) => {
                        return Err(AppError::Download(DownloadError::YtDlp(format!("curl failed: {}", e))));
                    }
                    Err(_) => {
                        return Err(AppError::Download(DownloadError::YtDlp(
                            "curl download timed out".to_string(),
                        )));
                    }
                }
            }
        }
        Ok(Err(e)) => {
            return Err(AppError::Download(DownloadError::YtDlp(format!("wget failed: {}", e))));
        }
        Err(_) => {
            return Err(AppError::Download(DownloadError::YtDlp(
                "wget download timed out".to_string(),
            )));
        }
    }

    // Set executable permissions using native API (avoids external command)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(ytdl_bin, std::fs::Permissions::from_mode(0o755)) {
            log::warn!("Failed to set yt-dlp permissions: {}", e);
        }
    }

    let new_version = get_current_version();
    log::info!("yt-dlp updated: {} → {}", old_version, new_version);

    Ok((old_version, new_version))
}

/// Checks and updates yt-dlp to the latest nightly version at bot startup.
pub async fn check_and_update_ytdlp() -> Result<(), AppError> {
    let old_version = get_current_version();
    log::info!("Current yt-dlp version: {}", old_version);
    log::info!("Updating yt-dlp to latest nightly build...");

    match download_nightly_ytdlp().await {
        Ok((old, new)) => {
            if old == new {
                log::info!("yt-dlp is already at the latest nightly version: {}", new);
            } else {
                log::info!("✅ yt-dlp updated successfully: {} → {}", old, new);
            }
        }
        Err(e) => {
            log::warn!("Failed to update yt-dlp: {}. Continuing with current version.", e);
        }
    }

    Ok(())
}

/// Starts the background yt-dlp auto-update task.
///
/// Updates yt-dlp every N hours to prevent 403 errors from YouTube.
pub fn start_auto_update_task() {
    STOP_AUTO_UPDATE.store(false, Ordering::SeqCst);

    tokio::spawn(async move {
        let interval = Duration::from_secs(AUTO_UPDATE_INTERVAL_HOURS * 60 * 60);

        log::info!(
            "🔄 yt-dlp auto-update task started (interval: {} hours)",
            AUTO_UPDATE_INTERVAL_HOURS
        );

        loop {
            // Wait for the interval
            tokio::time::sleep(interval).await;

            // Check the stop flag
            if STOP_AUTO_UPDATE.load(Ordering::SeqCst) {
                log::info!("yt-dlp auto-update task stopped");
                break;
            }

            log::info!("🔄 Running scheduled yt-dlp update...");

            match download_nightly_ytdlp().await {
                Ok((old, new)) => {
                    if old == new {
                        log::info!("yt-dlp is already at the latest version: {}", new);
                    } else {
                        log::info!("✅ yt-dlp auto-updated: {} → {}", old, new);
                    }
                }
                Err(e) => {
                    log::error!("❌ yt-dlp auto-update failed: {}", e);
                }
            }
        }
    });
}

/// Stops the background auto-update task.
pub fn stop_auto_update_task() {
    STOP_AUTO_UPDATE.store(true, Ordering::SeqCst);
    log::info!("yt-dlp auto-update task stop requested");
}

/// Checks whether a URL is supported by yt-dlp.
///
/// Performs a quick check to see whether yt-dlp can handle the given URL.
/// Uses `yt-dlp --dump-json` to verify without downloading.
///
/// # Arguments
///
/// * `url` - URL to check
///
/// # Returns
///
/// Returns `Ok(true)` if the URL is supported, `Ok(false)` if not,
/// or an error if the command fails.
pub async fn is_url_supported(url: &url::Url) -> Result<bool, AppError> {
    let ytdl_bin = &*config::YTDL_BIN;

    // Quick check via --dump-json (does not download the file)
    let check_result = timeout(
        std::time::Duration::from_secs(10), // 10 seconds for the check
        TokioCommand::new(ytdl_bin)
            .args(["--dump-json", "--no-playlist", url.as_str()])
            .output(),
    )
    .await;

    match check_result {
        Ok(Ok(output)) => {
            if output.status.success() {
                // Verify that the output contains at least minimal information
                let stdout = String::from_utf8_lossy(&output.stdout);
                Ok(stdout.contains("\"id\"") || stdout.contains("\"title\""))
            } else {
                Ok(false)
            }
        }
        Ok(Err(_)) => {
            // If the command failed to run, assume the URL is not supported
            Ok(false)
        }
        Err(_) => {
            // Timeout — assume the URL may be supported but the check took too long
            log::warn!("URL support check timed out for: {}", url);
            Ok(true) // Assume support on timeout
        }
    }
}

/// Returns the list of services supported by yt-dlp.
///
/// Uses `yt-dlp --list-extractors` to obtain the full list of supported extractors.
///
/// # Returns
///
/// Returns a vector of extractor name strings or an error.
pub async fn get_supported_extractors() -> Result<Vec<String>, AppError> {
    let ytdl_bin = &*config::YTDL_BIN;

    let output = timeout(
        std::time::Duration::from_secs(10),
        TokioCommand::new(ytdl_bin).arg("--list-extractors").output(),
    )
    .await
    .map_err(|_| {
        AppError::Download(DownloadError::YtDlp(
            "yt-dlp list-extractors command timed out".to_string(),
        ))
    })?
    .map_err(|e| {
        AppError::Download(DownloadError::YtDlp(format!(
            "Failed to execute yt-dlp --list-extractors: {}",
            e
        )))
    })?;

    if !output.status.success() {
        return Err(AppError::Download(DownloadError::YtDlp(
            "yt-dlp --list-extractors failed".to_string(),
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let extractors: Vec<String> = stdout
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();

    Ok(extractors)
}

/// Checks whether a specific service (VK, TikTok, Instagram, Twitch, Spotify) is supported.
///
/// # Arguments
///
/// * `service_name` - Service name (e.g. "vk", "tiktok", "instagram", "twitch", "spotify")
///
/// # Returns
///
/// Returns `true` if the service is supported, `false` otherwise.
pub async fn is_service_supported(service_name: &str) -> bool {
    match get_supported_extractors().await {
        Ok(extractors) => {
            let service_lower = service_name.to_lowercase();
            extractors
                .iter()
                .any(|extractor| extractor.to_lowercase().contains(&service_lower))
        }
        Err(e) => {
            log::warn!(
                "Failed to get supported extractors: {}. Assuming service is supported.",
                e
            );
            true // Assume support on error
        }
    }
}

/// Prints the current yt-dlp version.
///
/// # Returns
///
/// Returns `Ok(())` on success or an error on failure.
pub async fn print_ytdlp_version() -> Result<(), AppError> {
    let ytdl_bin = &*config::YTDL_BIN;

    log::info!("Checking yt-dlp version...");

    let version_output = Command::new(ytdl_bin)
        .arg("--version")
        .output()
        .map_err(|e| AppError::Download(DownloadError::YtDlp(format!("Failed to get yt-dlp version: {}", e))))?;

    let version = String::from_utf8_lossy(&version_output.stdout).trim().to_string();

    if version.is_empty() {
        return Err(AppError::Download(DownloadError::YtDlp(
            "yt-dlp is not installed or --version produced no output".to_string(),
        )));
    }

    println!("yt-dlp version: {}", version);
    log::info!("yt-dlp version: {}", version);

    Ok(())
}

/// Force-updates yt-dlp to the latest nightly version.
///
/// Uses nightly builds for best compatibility with YouTube.
///
/// # Returns
///
/// Returns `Ok(())` on success or an error on failure.
pub async fn force_update_ytdlp() -> Result<(), AppError> {
    log::info!("Force updating yt-dlp from nightly builds...");
    println!("Force updating yt-dlp to the latest nightly version...");

    let (old_version, new_version) = download_nightly_ytdlp().await?;

    println!("✅ yt-dlp updated: {} → {}", old_version, new_version);

    Ok(())
}
