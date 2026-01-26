use crate::core::config;
use crate::core::error::AppError;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::process::Command as TokioCommand;
use tokio::time::{timeout, Duration};

/// Интервал автообновления yt-dlp (6 часов)
const AUTO_UPDATE_INTERVAL_HOURS: u64 = 6;

/// URL для скачивания nightly билдов yt-dlp
const NIGHTLY_URL: &str = "https://github.com/yt-dlp/yt-dlp-nightly-builds/releases/latest/download/yt-dlp";

/// Флаг для остановки фонового обновления
static STOP_AUTO_UPDATE: AtomicBool = AtomicBool::new(false);

/// Получает текущую версию yt-dlp
pub fn get_current_version() -> String {
    let ytdl_bin = &*config::YTDL_BIN;
    Command::new(ytdl_bin)
        .arg("--version")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Скачивает и устанавливает последнюю nightly версию yt-dlp
async fn download_nightly_ytdlp() -> Result<(String, String), AppError> {
    let ytdl_bin = &*config::YTDL_BIN;
    let old_version = get_current_version();

    log::info!("Downloading yt-dlp nightly build...");

    // Скачиваем через wget
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
                // Пробуем curl как fallback
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
                            return Err(AppError::Download(format!(
                                "Failed to download yt-dlp nightly: {}",
                                stderr
                            )));
                        }
                    }
                    Ok(Err(e)) => {
                        return Err(AppError::Download(format!("curl failed: {}", e)));
                    }
                    Err(_) => {
                        return Err(AppError::Download("curl download timed out".to_string()));
                    }
                }
            }
        }
        Ok(Err(e)) => {
            return Err(AppError::Download(format!("wget failed: {}", e)));
        }
        Err(_) => {
            return Err(AppError::Download("wget download timed out".to_string()));
        }
    }

    // Устанавливаем права на выполнение
    let _ = TokioCommand::new("chmod").args(["a+rx", ytdl_bin]).output().await;

    let new_version = get_current_version();
    log::info!("yt-dlp updated: {} → {}", old_version, new_version);

    Ok((old_version, new_version))
}

/// Проверяет и обновляет yt-dlp до последней nightly версии при старте бота
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

/// Запускает фоновую задачу автообновления yt-dlp
///
/// Обновляет yt-dlp каждые N часов для предотвращения 403 ошибок от YouTube.
pub fn start_auto_update_task() {
    STOP_AUTO_UPDATE.store(false, Ordering::SeqCst);

    tokio::spawn(async move {
        let interval = Duration::from_secs(AUTO_UPDATE_INTERVAL_HOURS * 60 * 60);

        log::info!(
            "🔄 yt-dlp auto-update task started (interval: {} hours)",
            AUTO_UPDATE_INTERVAL_HOURS
        );

        loop {
            // Ждём интервал
            tokio::time::sleep(interval).await;

            // Проверяем флаг остановки
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

/// Останавливает фоновую задачу автообновления
pub fn stop_auto_update_task() {
    STOP_AUTO_UPDATE.store(true, Ordering::SeqCst);
    log::info!("yt-dlp auto-update task stop requested");
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
            .output(),
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
        TokioCommand::new(ytdl_bin).arg("--list-extractors").output(),
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
            extractors
                .iter()
                .any(|extractor| extractor.to_lowercase().contains(&service_lower))
        }
        Err(e) => {
            log::warn!(
                "Failed to get supported extractors: {}. Assuming service is supported.",
                e
            );
            true // В случае ошибки предполагаем поддержку
        }
    }
}

/// Выводит текущую версию yt-dlp
///
/// # Returns
///
/// Возвращает `Ok(())` при успехе или ошибку при неудаче.
pub async fn print_ytdlp_version() -> Result<(), AppError> {
    let ytdl_bin = &*config::YTDL_BIN;

    log::info!("Checking yt-dlp version...");

    let version_output = Command::new(ytdl_bin)
        .arg("--version")
        .output()
        .map_err(|e| AppError::Download(format!("Failed to get yt-dlp version: {}", e)))?;

    let version = String::from_utf8_lossy(&version_output.stdout).trim().to_string();

    if version.is_empty() {
        return Err(AppError::Download(
            "yt-dlp is not installed or --version produced no output".to_string(),
        ));
    }

    println!("yt-dlp version: {}", version);
    log::info!("yt-dlp version: {}", version);

    Ok(())
}

/// Принудительно обновляет yt-dlp до последней nightly версии
///
/// Использует nightly builds для лучшей совместимости с YouTube.
///
/// # Returns
///
/// Возвращает `Ok(())` при успехе или ошибку при неудаче.
pub async fn force_update_ytdlp() -> Result<(), AppError> {
    log::info!("Force updating yt-dlp from nightly builds...");
    println!("Force updating yt-dlp to the latest nightly version...");

    let (old_version, new_version) = download_nightly_ytdlp().await?;

    println!("✅ yt-dlp updated: {} → {}", old_version, new_version);

    Ok(())
}
