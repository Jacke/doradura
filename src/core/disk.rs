//! Disk space monitoring and validation
//!
//! Provides utilities for checking available disk space and preventing
//! downloads when disk is full.

use crate::core::config;
use crate::core::error::AppError;
use crate::core::metrics;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Minimum required disk space for downloads (500 MB)
pub const MIN_DISK_SPACE_BYTES: u64 = 500 * 1024 * 1024;

/// Warning threshold for disk space (1 GB)
pub const WARNING_DISK_SPACE_BYTES: u64 = 1024 * 1024 * 1024;

/// Critical threshold for disk space (500 MB)
pub const CRITICAL_DISK_SPACE_BYTES: u64 = 500 * 1024 * 1024;

/// Disk space check interval (5 minutes)
pub const DISK_CHECK_INTERVAL_SECS: u64 = 300;

/// Flag to stop background disk monitoring
static STOP_DISK_MONITOR: AtomicBool = AtomicBool::new(false);

/// Result of disk space check
#[derive(Debug, Clone)]
pub struct DiskSpaceInfo {
    /// Available space in bytes
    pub available_bytes: u64,
    /// Total space in bytes
    pub total_bytes: u64,
    /// Used percentage (0-100)
    pub used_percent: f64,
    /// Path that was checked
    pub path: String,
}

impl DiskSpaceInfo {
    /// Returns available space in GB
    pub fn available_gb(&self) -> f64 {
        self.available_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
    }

    /// Returns total space in GB
    pub fn total_gb(&self) -> f64 {
        self.total_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
    }

    /// Check if space is critically low
    pub fn is_critical(&self) -> bool {
        self.available_bytes < CRITICAL_DISK_SPACE_BYTES
    }

    /// Check if space is low (warning level)
    pub fn is_warning(&self) -> bool {
        self.available_bytes < WARNING_DISK_SPACE_BYTES
    }

    /// Check if there's enough space for a download
    pub fn has_enough_space(&self) -> bool {
        self.available_bytes >= MIN_DISK_SPACE_BYTES
    }
}

/// Get disk space information for a path using df command
///
/// This is a cross-platform approach that works on Linux and macOS.
pub fn get_disk_space(path: &str) -> Result<DiskSpaceInfo, AppError> {
    // Resolve the path (handle ~ expansion)
    let expanded_path = shellexpand::tilde(path).into_owned();
    let check_path = if Path::new(&expanded_path).exists() {
        expanded_path.clone()
    } else {
        // If path doesn't exist, use parent directory
        Path::new(&expanded_path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".to_string())
    };

    // Use df command to get disk space info
    let output = std::process::Command::new("df")
        .args(["-k", &check_path]) // -k for 1K blocks
        .output()
        .map_err(|e| AppError::Download(format!("Failed to run df command: {}", e)))?;

    if !output.status.success() {
        return Err(AppError::Download(format!(
            "df command failed for {}: {}",
            check_path,
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();

    // Skip header line, parse data line
    if lines.len() < 2 {
        return Err(AppError::Download("Unexpected df output format".to_string()));
    }

    // df output: Filesystem 1K-blocks Used Available Use% Mounted
    let parts: Vec<&str> = lines[1].split_whitespace().collect();
    if parts.len() < 4 {
        return Err(AppError::Download("Unexpected df output format".to_string()));
    }

    let total_kb: u64 = parts[1]
        .parse()
        .map_err(|_| AppError::Download("Failed to parse total blocks".to_string()))?;
    let available_kb: u64 = parts[3]
        .parse()
        .map_err(|_| AppError::Download("Failed to parse available blocks".to_string()))?;

    let total_bytes = total_kb * 1024;
    let available_bytes = available_kb * 1024;
    let used_bytes = total_bytes.saturating_sub(available_bytes);
    let used_percent = if total_bytes > 0 {
        (used_bytes as f64 / total_bytes as f64) * 100.0
    } else {
        0.0
    };

    Ok(DiskSpaceInfo {
        available_bytes,
        total_bytes,
        used_percent,
        path: check_path,
    })
}

/// Check if there's enough disk space for a download
///
/// Returns Ok(DiskSpaceInfo) if there's enough space, or an error with a user-friendly message.
pub fn check_disk_space_for_download() -> Result<DiskSpaceInfo, AppError> {
    let download_folder = &*config::DOWNLOAD_FOLDER;
    let info = get_disk_space(download_folder)?;

    if !info.has_enough_space() {
        log::error!(
            "🚨 Insufficient disk space: {:.2} GB available (need {:.2} GB)",
            info.available_gb(),
            MIN_DISK_SPACE_BYTES as f64 / (1024.0 * 1024.0 * 1024.0)
        );

        metrics::record_error("download", "disk_space_insufficient");

        return Err(AppError::Download(format!(
            "Недостаточно места на диске: {:.2} GB свободно",
            info.available_gb()
        )));
    }

    if info.is_warning() {
        log::warn!("⚠️  Low disk space warning: {:.2} GB available", info.available_gb());
    }

    Ok(info)
}

/// Check disk space and log status
///
/// This function is meant to be called periodically to monitor disk space.
pub async fn check_and_log_disk_space() {
    let download_folder = &*config::DOWNLOAD_FOLDER;

    match get_disk_space(download_folder) {
        Ok(info) => {
            if info.is_critical() {
                log::error!(
                    "🚨 CRITICAL: Disk space critically low: {:.2} GB available ({:.1}% used)",
                    info.available_gb(),
                    info.used_percent
                );
                metrics::record_error("system", "disk_space_critical");
            } else if info.is_warning() {
                log::warn!(
                    "⚠️  Disk space warning: {:.2} GB available ({:.1}% used)",
                    info.available_gb(),
                    info.used_percent
                );
            } else {
                log::debug!(
                    "💾 Disk space OK: {:.2} GB available ({:.1}% used)",
                    info.available_gb(),
                    info.used_percent
                );
            }
        }
        Err(e) => {
            log::error!("Failed to check disk space: {}", e);
        }
    }
}

/// Start background disk space monitoring task
///
/// Checks disk space every N minutes and logs warnings if space is low.
/// Returns JoinHandle that can be used to await task completion or check for panics.
pub fn start_disk_monitor_task() -> tokio::task::JoinHandle<()> {
    STOP_DISK_MONITOR.store(false, Ordering::SeqCst);

    let handle = tokio::spawn(async move {
        let interval = Duration::from_secs(DISK_CHECK_INTERVAL_SECS);

        log::info!(
            "💾 Disk space monitor started (interval: {} seconds)",
            DISK_CHECK_INTERVAL_SECS
        );

        // Initial check
        check_and_log_disk_space().await;

        loop {
            tokio::time::sleep(interval).await;

            if STOP_DISK_MONITOR.load(Ordering::SeqCst) {
                log::info!("Disk space monitor stopped");
                break;
            }

            check_and_log_disk_space().await;
        }
    });

    handle
}

/// Stop background disk space monitoring
pub fn stop_disk_monitor_task() {
    STOP_DISK_MONITOR.store(true, Ordering::SeqCst);
    log::info!("Disk space monitor stop requested");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_disk_space() {
        let result = get_disk_space("/tmp");
        assert!(result.is_ok(), "Failed to get disk space: {:?}", result.err());

        let info = result.unwrap();
        assert!(info.available_bytes > 0);
        assert!(info.total_bytes > 0);
        assert!(info.used_percent >= 0.0);
        assert!(info.used_percent <= 100.0);
    }

    #[test]
    fn test_disk_space_info_methods() {
        let info = DiskSpaceInfo {
            available_bytes: 2 * 1024 * 1024 * 1024, // 2 GB
            total_bytes: 10 * 1024 * 1024 * 1024,    // 10 GB
            used_percent: 80.0,
            path: "/tmp".to_string(),
        };

        assert!((info.available_gb() - 2.0).abs() < 0.01);
        assert!((info.total_gb() - 10.0).abs() < 0.01);
        assert!(!info.is_critical());
        assert!(!info.is_warning());
        assert!(info.has_enough_space());
    }

    #[test]
    fn test_disk_space_critical() {
        let info = DiskSpaceInfo {
            available_bytes: 100 * 1024 * 1024, // 100 MB
            total_bytes: 10 * 1024 * 1024 * 1024,
            used_percent: 99.0,
            path: "/tmp".to_string(),
        };

        assert!(info.is_critical());
        assert!(info.is_warning());
        assert!(!info.has_enough_space());
    }

    #[test]
    fn test_check_disk_space_for_download() {
        // This test checks that the function works (doesn't crash)
        // It may pass or fail depending on actual disk space
        let result = check_disk_space_for_download();
        // We just verify it doesn't panic
        let _ = result;
    }
}
