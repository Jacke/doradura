use crate::core::config;
use crate::core::error::AppError;
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
    let version_output = Command::new(ytdl_bin).arg("--version").output();

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
        TokioCommand::new(ytdl_bin).arg("-U").output(),
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
                if output.status.code() == Some(100) {
                    log::info!("yt-dlp is installed via pip. Attempting to update via pip...");

                    // Пытаемся обновить через pip или pip3
                    let pip_commands = vec!["pip3", "pip"];
                    let mut update_successful = false;
                    let mut last_error: Option<String> = None;

                    for pip_cmd in pip_commands {
                        log::debug!("Trying to update yt-dlp via {}...", pip_cmd);

                        let pip_update_result = timeout(
                            std::time::Duration::from_secs(60), // 60 секунд на обновление через pip
                            TokioCommand::new(pip_cmd)
                                .args(["install", "--upgrade", "yt-dlp"])
                                .output(),
                        )
                        .await;

                        match pip_update_result {
                            Ok(Ok(pip_output)) => {
                                if pip_output.status.success() {
                                    let pip_stdout = String::from_utf8_lossy(&pip_output.stdout);
                                    if pip_stdout.contains("Successfully installed")
                                        || pip_stdout.contains("Requirement already satisfied")
                                    {
                                        log::info!("yt-dlp updated successfully via {}", pip_cmd);
                                        update_successful = true;
                                        break;
                                    } else {
                                        log::info!("yt-dlp {} update: {}", pip_cmd, pip_stdout);
                                        update_successful = true;
                                        break;
                                    }
                                } else {
                                    let pip_stderr = String::from_utf8_lossy(&pip_output.stderr);
                                    let exit_code = pip_output.status.code();
                                    let error_msg =
                                        format!("{} failed with exit code {:?}: {}", pip_cmd, exit_code, pip_stderr);
                                    log::debug!("{} update failed: {}", pip_cmd, error_msg);
                                    last_error = Some(error_msg);
                                    // Пробуем следующую команду
                                    continue;
                                }
                            }
                            Ok(Err(e)) => {
                                last_error = Some(format!("{} command not found or failed to execute: {}", pip_cmd, e));
                                log::debug!("{} command error: {}", pip_cmd, e);
                                // Пробуем следующую команду
                                continue;
                            }
                            Err(_) => {
                                last_error = Some(format!("{} update timed out after 60 seconds", pip_cmd));
                                log::debug!("{} update timed out", pip_cmd);
                                // Пробуем следующую команду
                                continue;
                            }
                        }
                    }

                    if !update_successful {
                        if let Some(error) = last_error {
                            log::warn!("Failed to update yt-dlp via pip/pip3. Last error: {}. You may need to run 'pip install --upgrade yt-dlp' or 'pip3 install --upgrade yt-dlp' manually (may require sudo).", error);
                        } else {
                            log::warn!("Failed to update yt-dlp via pip/pip3. You may need to run 'pip install --upgrade yt-dlp' or 'pip3 install --upgrade yt-dlp' manually (may require sudo).");
                        }
                    }
                } else {
                    log::warn!(
                        "yt-dlp update check failed (exit code: {:?}): {}",
                        output.status.code(),
                        stderr
                    );
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
    let ytdl_bin = &*config::YTDL_BIN;

    log::info!("Force updating yt-dlp from nightly builds...");
    println!("Force updating yt-dlp to the latest nightly version...");

    // Получаем текущую версию для сравнения
    let old_version = Command::new(ytdl_bin)
        .arg("--version")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    log::info!("Current yt-dlp version: {}", old_version);

    // Скачиваем последний nightly билд напрямую
    let nightly_url = "https://github.com/yt-dlp/yt-dlp-nightly-builds/releases/latest/download/yt-dlp";

    let download_result = timeout(
        std::time::Duration::from_secs(120), // 2 минуты на загрузку
        TokioCommand::new("wget")
            .args(["-q", "-O", ytdl_bin, nightly_url])
            .output(),
    )
    .await;

    match download_result {
        Ok(Ok(output)) => {
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                // Пробуем curl как fallback
                log::info!("wget failed, trying curl...");

                let curl_result = timeout(
                    std::time::Duration::from_secs(120),
                    TokioCommand::new("curl")
                        .args(["-fsSL", "-o", ytdl_bin, nightly_url])
                        .output(),
                )
                .await;

                match curl_result {
                    Ok(Ok(curl_output)) => {
                        if !curl_output.status.success() {
                            let curl_stderr = String::from_utf8_lossy(&curl_output.stderr);
                            return Err(AppError::Download(format!(
                                "Failed to download yt-dlp nightly: wget: {}, curl: {}",
                                stderr, curl_stderr
                            )));
                        }
                    }
                    Ok(Err(e)) => {
                        return Err(AppError::Download(format!("Failed to execute curl: {}", e)));
                    }
                    Err(_) => {
                        return Err(AppError::Download(
                            "curl download timed out after 2 minutes".to_string(),
                        ));
                    }
                }
            }

            // Устанавливаем права на выполнение
            let chmod_result = TokioCommand::new("chmod").args(["a+rx", ytdl_bin]).output().await;

            if let Err(e) = chmod_result {
                log::warn!("Failed to set yt-dlp permissions: {}", e);
            }

            // Проверяем новую версию
            let new_version = Command::new(ytdl_bin)
                .arg("--version")
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_else(|_| "unknown".to_string());

            log::info!("New yt-dlp version: {}", new_version);
            println!("✅ yt-dlp updated: {} → {}", old_version, new_version);

            Ok(())
        }
        Ok(Err(e)) => Err(AppError::Download(format!("Failed to execute wget: {}", e))),
        Err(_) => Err(AppError::Download(
            "yt-dlp download timed out after 2 minutes".to_string(),
        )),
    }
}
