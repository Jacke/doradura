use anyhow::Result;
use chrono::{DateTime, Utc};
use std::fs;
use std::path::{Path, PathBuf};

/// Maximum number of stored backups
const MAX_BACKUPS: usize = 30;

/// Base directory for backups
const BACKUP_DIR: &str = "backups";

/// Creates the backup directory if it does not exist
fn ensure_backup_dir() -> Result<PathBuf> {
    let backup_dir = PathBuf::from(BACKUP_DIR);
    if !backup_dir.exists() {
        fs::create_dir_all(&backup_dir)?;
        log::info!("Created backup directory: {}", backup_dir.display());
    }
    Ok(backup_dir)
}

/// Creates a database backup
///
/// # Arguments
///
/// * `db_path` - Path to the database file
///
/// # Returns
///
/// Returns the path to the created backup or an error
pub fn create_backup(db_path: &str) -> Result<PathBuf> {
    let backup_dir = ensure_backup_dir()?;

    // Generate filename with timestamp
    let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
    let db_name = Path::new(db_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("database.sqlite");
    let backup_filename = format!("{}_{}", timestamp, db_name);
    let backup_path = backup_dir.join(backup_filename);

    // Copy the database file
    fs::copy(db_path, &backup_path)?;
    log::info!("Created backup: {}", backup_path.display());

    // Clean up old backups
    cleanup_old_backups(&backup_dir)?;

    Ok(backup_path)
}

/// Deletes old backups, keeping only the latest MAX_BACKUPS
fn cleanup_old_backups(backup_dir: &Path) -> Result<()> {
    let mut backups: Vec<(PathBuf, DateTime<Utc>)> = Vec::new();

    // Collect all backups with their timestamps
    if backup_dir.is_dir() {
        for entry in fs::read_dir(backup_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("sqlite") {
                // Try to extract timestamp from the filename
                if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                    // Format: YYYYMMDD_HHMMSS_database.sqlite
                    if let Some(timestamp_part) = file_name.split('_').take(2).collect::<Vec<_>>().join("_").get(0..15)
                    {
                        if let Ok(dt) = DateTime::parse_from_str(timestamp_part, "%Y%m%d_%H%M%S") {
                            backups.push((path, dt.with_timezone(&Utc)));
                        }
                    }
                }
            }
        }
    }

    // Sort by time (newest first)
    backups.sort_by(|a, b| b.1.cmp(&a.1));

    // Delete old backups
    if backups.len() > MAX_BACKUPS {
        for (path, _) in backups.iter().skip(MAX_BACKUPS) {
            if let Err(e) = fs::remove_file(path) {
                log::warn!("Failed to remove old backup {}: {}", path.display(), e);
            } else {
                log::info!("Removed old backup: {}", path.display());
            }
        }
    }

    Ok(())
}

/// Gets the list of all backups
pub fn list_backups() -> Result<Vec<(PathBuf, DateTime<Utc>)>> {
    let backup_dir = ensure_backup_dir()?;
    let mut backups: Vec<(PathBuf, DateTime<Utc>)> = Vec::new();

    if backup_dir.is_dir() {
        for entry in fs::read_dir(&backup_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("sqlite") {
                if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                    if let Some(timestamp_part) = file_name.split('_').take(2).collect::<Vec<_>>().join("_").get(0..15)
                    {
                        if let Ok(dt) = DateTime::parse_from_str(timestamp_part, "%Y%m%d_%H%M%S") {
                            backups.push((path, dt.with_timezone(&Utc)));
                        }
                    }
                }
            }
        }
    }

    // Sort by time (newest first)
    backups.sort_by(|a, b| b.1.cmp(&a.1));

    Ok(backups)
}

/// Restores a database from a backup
///
/// # Arguments
///
/// * `backup_path` - Path to the backup file
/// * `db_path` - Path to the database file to restore
///
/// # Returns
///
/// Returns Ok(()) on success or an error
pub fn restore_backup(backup_path: &Path, db_path: &str) -> Result<()> {
    if !backup_path.exists() {
        return Err(anyhow::anyhow!("Backup file does not exist: {}", backup_path.display()));
    }

    // Copy the backup over the database file
    fs::copy(backup_path, db_path)?;
    log::info!("Restored database from backup: {}", backup_path.display());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_max_backups_constant() {
        assert_eq!(MAX_BACKUPS, 30);
    }

    #[test]
    fn test_backup_dir_constant() {
        assert_eq!(BACKUP_DIR, "backups");
    }

    #[test]
    fn test_restore_backup_nonexistent() {
        let result = restore_backup(Path::new("/nonexistent/backup.sqlite"), "/tmp/db.sqlite");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn test_restore_backup_success() {
        let temp_dir = TempDir::new().unwrap();

        // Create a backup file
        let backup_path = temp_dir.path().join("backup.sqlite");
        fs::write(&backup_path, b"backup content").unwrap();

        // Destination path
        let db_path = temp_dir.path().join("restored.sqlite");

        let result = restore_backup(&backup_path, db_path.to_str().unwrap());
        assert!(result.is_ok());

        // Verify the restored file has the correct content
        let restored_content = fs::read(&db_path).unwrap();
        assert_eq!(restored_content, b"backup content");
    }

    #[test]
    fn test_cleanup_old_backups_empty_dir() {
        let temp_dir = TempDir::new().unwrap();
        let result = cleanup_old_backups(temp_dir.path());
        assert!(result.is_ok());
    }

    #[test]
    fn test_cleanup_old_backups_nonexistent_dir() {
        let result = cleanup_old_backups(Path::new("/nonexistent/dir"));
        assert!(result.is_ok()); // Should not fail, just skip
    }
}
