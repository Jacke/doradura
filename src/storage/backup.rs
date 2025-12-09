use anyhow::Result;
use chrono::{DateTime, Utc};
use std::fs;
use std::path::{Path, PathBuf};

/// Максимальное количество хранимых бэкапов
const MAX_BACKUPS: usize = 30;

/// Базовая директория для бэкапов
const BACKUP_DIR: &str = "backups";

/// Создает директорию для бэкапов если её нет
fn ensure_backup_dir() -> Result<PathBuf> {
    let backup_dir = PathBuf::from(BACKUP_DIR);
    if !backup_dir.exists() {
        fs::create_dir_all(&backup_dir)?;
        log::info!("Created backup directory: {}", backup_dir.display());
    }
    Ok(backup_dir)
}

/// Создает бэкап базы данных
///
/// # Arguments
///
/// * `db_path` - Путь к файлу базы данных
///
/// # Returns
///
/// Возвращает путь к созданному бэкапу или ошибку
pub fn create_backup(db_path: &str) -> Result<PathBuf> {
    let backup_dir = ensure_backup_dir()?;

    // Генерируем имя файла с timestamp
    let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
    let db_name = Path::new(db_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("database.sqlite");
    let backup_filename = format!("{}_{}", timestamp, db_name);
    let backup_path = backup_dir.join(backup_filename);

    // Копируем файл базы данных
    fs::copy(db_path, &backup_path)?;
    log::info!("Created backup: {}", backup_path.display());

    // Очищаем старые бэкапы
    cleanup_old_backups(&backup_dir)?;

    Ok(backup_path)
}

/// Удаляет старые бэкапы, оставляя только последние MAX_BACKUPS
fn cleanup_old_backups(backup_dir: &Path) -> Result<()> {
    let mut backups: Vec<(PathBuf, DateTime<Utc>)> = Vec::new();

    // Собираем все бэкапы с их временными метками
    if backup_dir.is_dir() {
        for entry in fs::read_dir(backup_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("sqlite") {
                // Пытаемся извлечь timestamp из имени файла
                if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                    // Формат: YYYYMMDD_HHMMSS_database.sqlite
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

    // Сортируем по времени (новые первыми)
    backups.sort_by(|a, b| b.1.cmp(&a.1));

    // Удаляем старые бэкапы
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

/// Получает список всех бэкапов
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

    // Сортируем по времени (новые первыми)
    backups.sort_by(|a, b| b.1.cmp(&a.1));

    Ok(backups)
}

/// Восстанавливает базу данных из бэкапа
///
/// # Arguments
///
/// * `backup_path` - Путь к файлу бэкапа
/// * `db_path` - Путь к файлу базы данных для восстановления
///
/// # Returns
///
/// Возвращает Ok(()) при успехе или ошибку
pub fn restore_backup(backup_path: &Path, db_path: &str) -> Result<()> {
    if !backup_path.exists() {
        return Err(anyhow::anyhow!("Backup file does not exist: {}", backup_path.display()));
    }

    // Копируем бэкап на место базы данных
    fs::copy(backup_path, db_path)?;
    log::info!("Restored database from backup: {}", backup_path.display());

    Ok(())
}
