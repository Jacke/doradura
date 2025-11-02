use crate::config;
use crate::error::AppError;
use std::process::Command;
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;

/// Проверяет и обновляет yt-dlp до последней версии при старте бота
/// 
/// Выполняет проверку версии yt-dlp и обновляет её если доступна новая версия.
/// Использует команду `yt-dlp -U` для автоматического обновления.
/// 
/// # Returns
/// 
/// Возвращает `Ok(())` при успехе или ошибку при неудаче.
/// 
/// # Behavior
/// 
/// - Проверяет наличие yt-dlp в системе
/// - Пытается обновить yt-dlp через `yt-dlp -U`
/// - Логирует результаты обновления
pub async fn check_and_update_ytdlp() -> Result<(), AppError> {
    let ytdl_bin = &*config::YTDL_BIN;
    
    log::info!("Checking yt-dlp version...");
    
    // Проверяем текущую версию
    let version_output = Command::new(ytdl_bin)
        .arg("--version")
        .output();
    
    match version_output {
        Ok(output) => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            log::info!("Current yt-dlp version: {}", version);
        }
        Err(e) => {
            log::warn!("Failed to get yt-dlp version: {}. Will try to update anyway.", e);
        }
    }
    
    // Пытаемся обновить yt-dlp
    log::info!("Checking for yt-dlp updates...");
    
    let update_result = timeout(
        std::time::Duration::from_secs(30), // 30 секунд на обновление
        TokioCommand::new(ytdl_bin)
            .arg("-U")
            .output()
    )
    .await;
    
    match update_result {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            
            if output.status.success() {
                if stdout.contains("up to date") || stdout.contains("up-to-date") {
                    log::info!("yt-dlp is already up to date");
                } else if stdout.contains("Updated") || stdout.contains("updated") {
                    log::info!("yt-dlp updated successfully: {}", stdout);
                } else {
                    log::info!("yt-dlp update check completed: {}", stdout);
                }
            } else {
                // Код выхода 100 означает, что yt-dlp установлен через pip
                // Это нормальная ситуация, не нужно показывать предупреждение
                if output.status.code() == Some(100) {
                    log::info!("yt-dlp is installed via pip. Use 'pip install --upgrade yt-dlp' to update.");
                } else {
                    log::warn!("yt-dlp update check failed (exit code: {:?}): {}", output.status.code(), stderr);
                    // Не считаем это критической ошибкой - может быть проблема с сетью или правами
                }
            }
        }
        Ok(Err(e)) => {
            log::warn!("Failed to execute yt-dlp update: {}. Continuing anyway.", e);
        }
        Err(_) => {
            log::warn!("yt-dlp update check timed out. Continuing anyway.");
        }
    }
    
    Ok(())
}

/// Проверяет, поддерживается ли URL yt-dlp
/// 
/// Выполняет быструю проверку, может ли yt-dlp обработать данный URL.
/// Использует команду `yt-dlp --dump-json` для проверки без скачивания.
/// 
/// # Arguments
/// 
/// * `url` - URL для проверки
/// 
/// # Returns
/// 
/// Возвращает `Ok(true)` если URL поддерживается, `Ok(false)` если нет,
/// или ошибку при выполнении команды.
pub async fn is_url_supported(url: &url::Url) -> Result<bool, AppError> {
    let ytdl_bin = &*config::YTDL_BIN;
    
    // Быстрая проверка через --dump-json (не скачивает файл)
    let check_result = timeout(
        std::time::Duration::from_secs(10), // 10 секунд на проверку
        TokioCommand::new(ytdl_bin)
            .args(["--dump-json", "--no-playlist", url.as_str()])
            .output()
    )
    .await;
    
    match check_result {
        Ok(Ok(output)) => {
            if output.status.success() {
                // Проверяем, что в выводе есть хотя бы минимальная информация
                let stdout = String::from_utf8_lossy(&output.stdout);
                Ok(stdout.contains("\"id\"") || stdout.contains("\"title\""))
            } else {
                Ok(false)
            }
        }
        Ok(Err(_)) => {
            // Если команда не выполнилась, предполагаем что URL не поддерживается
            Ok(false)
        }
        Err(_) => {
            // Таймаут - считаем что URL может быть поддержан, но проверка заняла слишком долго
            log::warn!("URL support check timed out for: {}", url);
            Ok(true) // Предполагаем поддержку при таймауте
        }
    }
}

/// Получает список поддерживаемых сервисов yt-dlp
/// 
/// Использует команду `yt-dlp --list-extractors` для получения списка всех поддерживаемых экстракторов.
/// 
/// # Returns
/// 
/// Возвращает вектор строк с названиями поддерживаемых сервисов или ошибку.
pub async fn get_supported_extractors() -> Result<Vec<String>, AppError> {
    let ytdl_bin = &*config::YTDL_BIN;
    
    let output = timeout(
        std::time::Duration::from_secs(10),
        TokioCommand::new(ytdl_bin)
            .arg("--list-extractors")
            .output()
    )
    .await
    .map_err(|_| AppError::Download("yt-dlp list-extractors command timed out".to_string()))?
    .map_err(|e| AppError::Download(format!("Failed to execute yt-dlp --list-extractors: {}", e)))?;
    
    if !output.status.success() {
        return Err(AppError::Download("yt-dlp --list-extractors failed".to_string()));
    }
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    let extractors: Vec<String> = stdout
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    
    Ok(extractors)
}

/// Проверяет, поддерживается ли конкретный сервис (VK, TikTok, Instagram, Twitch, Spotify)
/// 
/// # Arguments
/// 
/// * `service_name` - Название сервиса (например, "vk", "tiktok", "instagram", "twitch", "spotify")
/// 
/// # Returns
/// 
/// Возвращает `Ok(true)` если сервис поддерживается, `Ok(false)` если нет.
pub async fn is_service_supported(service_name: &str) -> bool {
    match get_supported_extractors().await {
        Ok(extractors) => {
            let service_lower = service_name.to_lowercase();
            extractors.iter().any(|extractor| {
                extractor.to_lowercase().contains(&service_lower)
            })
        }
        Err(e) => {
            log::warn!("Failed to get supported extractors: {}. Assuming service is supported.", e);
            true // В случае ошибки предполагаем поддержку
        }
    }
}

