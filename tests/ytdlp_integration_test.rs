use std::path::PathBuf;
/// Интеграционный тест для проверки работоспособности yt-dlp
///
/// Этот тест проверяет:
/// - Установлен ли yt-dlp
/// - Работает ли скачивание видео
/// - Работают ли cookies (если настроены)
/// - Правильно ли обрабатываются ошибки
///
/// Запуск: cargo test --test ytdlp_integration_test -- --nocapture --test-threads=1
/// Запуск конкретного теста: cargo test --test ytdlp_integration_test test_ytdlp_download_with_metadata -- --nocapture
use std::process::{Command, Stdio};
use std::time::Duration;
use std::{env, fs};

/// Проверяет наличие команды в PATH
fn command_exists(bin: &str) -> bool {
    Command::new("bash")
        .arg("-lc")
        .arg(format!("command -v {} >/dev/null 2>&1", bin))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Возвращает путь к временной директории для тестов
fn get_test_downloads_dir() -> PathBuf {
    let tmp_dir = env::temp_dir().join("doradura_ytdlp_tests");
    let _ = fs::create_dir_all(&tmp_dir);
    tmp_dir
}

/// Очищает временную директорию после теста
fn cleanup_test_dir(dir: &PathBuf) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Получает путь к файлу cookies из переменной окружения
fn get_cookies_file() -> Option<String> {
    env::var("YTDL_COOKIES_FILE").ok()
}

/// Получает название браузера для cookies из переменной окружения
fn get_cookies_browser() -> Option<String> {
    env::var("YTDL_COOKIES_BROWSER").ok()
}

/// Тест 1: Проверка установки yt-dlp и ffmpeg
#[test]
fn test_ytdlp_installed() {
    println!("=== Проверка установки yt-dlp ===");

    let ytdlp_exists = command_exists("yt-dlp");
    let ffmpeg_exists = command_exists("ffmpeg");
    let ffprobe_exists = command_exists("ffprobe");

    println!(
        "✓ yt-dlp: {}",
        if ytdlp_exists {
            "установлен"
        } else {
            "НЕ УСТАНОВЛЕН"
        }
    );
    println!(
        "✓ ffmpeg: {}",
        if ffmpeg_exists {
            "установлен"
        } else {
            "НЕ УСТАНОВЛЕН"
        }
    );
    println!(
        "✓ ffprobe: {}",
        if ffprobe_exists {
            "установлен"
        } else {
            "НЕ УСТАНОВЛЕН"
        }
    );

    if !ytdlp_exists {
        println!("\n❌ ОШИБКА: yt-dlp не установлен!");
        println!("Установите: pip3 install yt-dlp");
    }

    if !ffmpeg_exists || !ffprobe_exists {
        println!("\n❌ ОШИБКА: ffmpeg/ffprobe не установлен!");
        println!("Установите: brew install ffmpeg (macOS) или apt install ffmpeg (Linux)");
    }

    assert!(ytdlp_exists, "yt-dlp должен быть установлен");
    assert!(ffmpeg_exists, "ffmpeg должен быть установлен");
    assert!(ffprobe_exists, "ffprobe должен быть установлен");
}

/// Тест 2: Проверка версии yt-dlp
#[test]
fn test_ytdlp_version() {
    if !command_exists("yt-dlp") {
        println!("⚠️  yt-dlp не установлен, пропускаем тест");
        return;
    }

    println!("=== Проверка версии yt-dlp ===");

    let output = Command::new("yt-dlp")
        .arg("--version")
        .output()
        .expect("Не удалось запустить yt-dlp --version");

    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    println!("✓ Версия yt-dlp: {}", version);

    assert!(!version.is_empty(), "Не удалось получить версию yt-dlp");
}

/// Тест 3: Проверка конфигурации cookies
#[test]
fn test_cookies_configuration() {
    println!("=== Проверка конфигурации cookies ===");

    let cookies_file = get_cookies_file();
    let cookies_browser = get_cookies_browser();

    match (&cookies_file, &cookies_browser) {
        (Some(file), _) => {
            println!("✓ Используется файл cookies: {}", file);

            // Проверяем существование файла
            if std::path::Path::new(file).exists() {
                println!("✓ Файл существует");

                // Проверяем размер файла
                if let Ok(metadata) = fs::metadata(file) {
                    println!("✓ Размер файла: {} байт", metadata.len());
                    assert!(metadata.len() > 0, "Файл cookies пустой");
                }
            } else {
                println!("❌ ОШИБКА: Файл cookies не найден по пути: {}", file);
                panic!("Файл cookies не существует");
            }
        }
        (None, Some(browser)) => {
            println!("✓ Используется браузер для cookies: {}", browser);
            println!("⚠️  ВНИМАНИЕ: На macOS требуется Full Disk Access для извлечения cookies из браузера");
            println!("   Рекомендуется использовать файл cookies вместо браузера");
        }
        (None, None) => {
            println!("❌ ОШИБКА: Cookies не настроены!");
            println!("\nДля работы с YouTube необходимо настроить cookies:");
            println!("1. Экспортируйте cookies из браузера в файл");
            println!(
                "2. Установите переменную окружения: export YTDL_COOKIES_FILE=/path/to/cookies.txt"
            );
            println!("3. Или используйте браузер: export YTDL_COOKIES_BROWSER=chrome");
            println!("\nСм. документацию: MACOS_COOKIES_FIX.md");

            // Это предупреждение, не фейлим тест
            eprintln!("\n⚠️  Без cookies большинство YouTube видео не будут скачиваться!");
        }
    }
}

/// Тест 4: Проверка получения метаданных с публичного видео
#[test]
#[ignore] // Требует сетевого подключения
fn test_ytdlp_get_metadata() {
    if !command_exists("yt-dlp") {
        println!("⚠️  yt-dlp не установлен, пропускаем тест");
        return;
    }

    println!("=== Проверка получения метаданных видео ===");

    // Используем короткое публичное видео
    let test_url = "https://www.youtube.com/watch?v=jNQXAC9IVRw"; // "Me at the zoo" - первое видео на YouTube

    let mut cmd = Command::new("yt-dlp");
    cmd.args(["--get-title", "--no-playlist"]);

    // Добавляем cookies если есть
    if let Some(cookies_file) = get_cookies_file() {
        cmd.args(["--cookies", &cookies_file]);
        println!("✓ Используется файл cookies: {}", cookies_file);
    } else if let Some(browser) = get_cookies_browser() {
        cmd.args(["--cookies-from-browser", &browser]);
        println!("✓ Используется браузер для cookies: {}", browser);
    }

    cmd.arg(test_url);

    let output = cmd.output().expect("Не удалось запустить yt-dlp");

    if output.status.success() {
        let title = String::from_utf8_lossy(&output.stdout).trim().to_string();
        println!("✓ Получен title: {}", title);
        assert!(!title.is_empty(), "Title не должен быть пустым");
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        println!("❌ ОШИБКА при получении метаданных:");
        println!("{}", stderr);

        // Анализируем ошибку
        if stderr.contains("Please sign in") || stderr.contains("cookies") {
            println!("\n💡 Решение: Настройте cookies (см. MACOS_COOKIES_FIX.md)");
        }
        if stderr.contains("PO Token") {
            println!("\n💡 Решение: Обновите yt-dlp до последней версии");
        }

        panic!("Не удалось получить метаданные видео");
    }
}

/// Тест 5: Скачивание аудио с проверкой успешности
#[test]
#[ignore] // Требует сетевого подключения
fn test_ytdlp_download_audio() {
    if !command_exists("yt-dlp") || !command_exists("ffmpeg") {
        println!("⚠️  yt-dlp или ffmpeg не установлен, пропускаем тест");
        return;
    }

    println!("=== Тест скачивания аудио ===");

    let tmp_dir = get_test_downloads_dir();
    let output_file = tmp_dir.join("test_audio.mp3");

    // Очищаем старые файлы
    cleanup_test_dir(&tmp_dir);

    // Используем короткое публичное видео
    let test_url = "https://www.youtube.com/watch?v=jNQXAC9IVRw"; // ~19 секунд

    let mut cmd = Command::new("yt-dlp");
    cmd.args([
        "-o",
        output_file.to_str().unwrap(),
        "--extract-audio",
        "--audio-format",
        "mp3",
        "--audio-quality",
        "0",
        "--no-playlist",
    ]);

    // Добавляем cookies если есть
    if let Some(cookies_file) = get_cookies_file() {
        cmd.args(["--cookies", &cookies_file]);
        println!("✓ Используется файл cookies: {}", cookies_file);
    } else if let Some(browser) = get_cookies_browser() {
        cmd.args(["--cookies-from-browser", &browser]);
        println!("✓ Используется браузер для cookies: {}", browser);
    } else {
        println!("⚠️  Cookies не настроены, скачивание может не работать");
    }

    // Добавляем настройки клиента
    // Используем android клиент который не требует PO Token
    let player_client = "youtube:player_client=android";

    cmd.args([
        "--extractor-args",
        player_client,
        "--no-check-certificate",
        test_url,
    ]);

    println!("Запуск команды: {:?}", cmd);
    let output = cmd
        .stdout(Stdio::inherit())
        .stderr(Stdio::piped())
        .output()
        .expect("Не удалось запустить yt-dlp");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        println!("\n❌ ОШИБКА при скачивании:");
        println!("{}", stderr);

        // Детальный анализ ошибок
        if stderr.contains("Please sign in") {
            println!("\n🔴 ПРОБЛЕМА: Требуется аутентификация");
            println!("💡 РЕШЕНИЕ:");
            println!("   1. Экспортируйте cookies из браузера");
            println!("   2. Установите: export YTDL_COOKIES_FILE=./youtube_cookies.txt");
            println!("   3. Перезапустите тест");
            println!("\n   Подробная инструкция: MACOS_COOKIES_FIX.md");
        }

        if stderr.contains("PO Token") || stderr.contains("GVS PO Token") {
            println!("\n🔴 ПРОБЛЕМА: Требуется PO Token (новое требование YouTube)");
            println!("💡 РЕШЕНИЕ:");
            println!("   1. Обновите yt-dlp: pip3 install -U yt-dlp");
            println!("   2. Убедитесь что используете cookies");
        }

        if stderr.contains("HTTP Error 403") || stderr.contains("bot detection") {
            println!("\n🔴 ПРОБЛЕМА: YouTube заблокировал запрос (обнаружен бот)");
            println!("💡 РЕШЕНИЕ:");
            println!("   1. Обязательно используйте cookies");
            println!("   2. Попробуйте другой player_client");
        }

        if stderr.contains("formats have been skipped") {
            println!("\n⚠️  ВНИМАНИЕ: Некоторые форматы пропущены");
            println!("   Это нормально, продолжаем скачивание доступных форматов");
        }

        panic!("Скачивание не удалось");
    }

    // Даем время на завершение ffmpeg конвертации
    std::thread::sleep(Duration::from_secs(2));

    // Проверяем что файл создан и не пустой
    assert!(
        output_file.exists(),
        "Файл не был создан: {:?}",
        output_file
    );

    let metadata = fs::metadata(&output_file).expect("Не удалось получить метаданные файла");
    println!("✓ Файл создан: {:?}", output_file);
    println!(
        "✓ Размер файла: {} байт ({:.2} MB)",
        metadata.len(),
        metadata.len() as f64 / 1024.0 / 1024.0
    );

    assert!(metadata.len() > 0, "Файл пустой");
    assert!(
        metadata.len() > 10000,
        "Файл слишком маленький (возможно поврежден)"
    );

    // Очищаем
    cleanup_test_dir(&tmp_dir);
    println!("✓ Тест успешно завершен");
}

/// Тест 6: Проверка обработки ошибок (невалидный URL)
#[test]
#[ignore]
fn test_ytdlp_invalid_url() {
    if !command_exists("yt-dlp") {
        println!("⚠️  yt-dlp не установлен, пропускаем тест");
        return;
    }

    println!("=== Тест обработки невалидного URL ===");

    let invalid_url = "https://www.youtube.com/watch?v=INVALID_VIDEO_ID_12345";

    let output = Command::new("yt-dlp")
        .args(["--get-title", "--no-playlist", invalid_url])
        .output()
        .expect("Не удалось запустить yt-dlp");

    // Ожидаем что команда завершится с ошибкой
    assert!(
        !output.status.success(),
        "Команда должна была завершиться с ошибкой для невалидного URL"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("✓ Ожидаемая ошибка получена:");
    println!("{}", stderr);

    // Проверяем что ошибка содержит релевантную информацию
    assert!(
        stderr.contains("ERROR")
            || stderr.contains("Video unavailable")
            || stderr.contains("not available"),
        "Ошибка должна содержать информацию о недоступности видео"
    );
}

/// Тест 7: Проверка скачивания с разными настройками качества
#[test]
#[ignore]
fn test_ytdlp_different_qualities() {
    if !command_exists("yt-dlp") || !command_exists("ffmpeg") {
        println!("⚠️  yt-dlp или ffmpeg не установлен, пропускаем тест");
        return;
    }

    println!("=== Тест скачивания с разными качествами ===");

    let tmp_dir = get_test_downloads_dir();
    cleanup_test_dir(&tmp_dir);

    let test_url = "https://www.youtube.com/watch?v=jNQXAC9IVRw";
    let qualities = vec![("320k", "320k"), ("192k", "192k"), ("128k", "128k")];

    for (name, bitrate) in qualities {
        println!("\n--- Тест качества: {} ---", name);
        let output_file = tmp_dir.join(format!("test_audio_{}.mp3", name));

        let mut cmd = Command::new("yt-dlp");
        cmd.args([
            "-o",
            output_file.to_str().unwrap(),
            "--extract-audio",
            "--audio-format",
            "mp3",
            "--audio-quality",
            "0",
            "--no-playlist",
            "--postprocessor-args",
            &format!("-acodec libmp3lame -b:a {}", bitrate),
        ]);

        // Добавляем cookies если есть
        if let Some(cookies_file) = get_cookies_file() {
            cmd.args(["--cookies", &cookies_file]);
        } else if let Some(browser) = get_cookies_browser() {
            cmd.args(["--cookies-from-browser", &browser]);
        }

        cmd.arg(test_url);

        let output = cmd
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .expect("Не удалось запустить yt-dlp");

        if output.status.success() {
            std::thread::sleep(Duration::from_secs(2));

            if output_file.exists() {
                let size = fs::metadata(&output_file).unwrap().len();
                println!("✓ Качество {}: {} байт", name, size);
            } else {
                println!("⚠️  Файл не создан для качества {}", name);
            }
        } else {
            println!("⚠️  Скачивание не удалось для качества {}", name);
        }
    }

    cleanup_test_dir(&tmp_dir);
    println!("\n✓ Тест разных качеств завершен");
}

/// Вспомогательная функция: Полная диагностика системы
#[test]
fn test_full_diagnostics() {
    println!("\n╔════════════════════════════════════════════════════════════════╗");
    println!("║         ПОЛНАЯ ДИАГНОСТИКА СИСТЕМЫ СКАЧИВАНИЯ                 ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");

    // 1. Проверка инструментов
    println!("📦 1. УСТАНОВЛЕННЫЕ ИНСТРУМЕНТЫ:");
    let tools = vec![
        ("yt-dlp", command_exists("yt-dlp")),
        ("ffmpeg", command_exists("ffmpeg")),
        ("ffprobe", command_exists("ffprobe")),
    ];

    for (tool, exists) in &tools {
        let status = if *exists {
            "✅ Установлен"
        } else {
            "❌ НЕ УСТАНОВЛЕН"
        };
        println!("   {} : {}", tool, status);
    }

    // 2. Версии
    println!("\n📋 2. ВЕРСИИ:");
    if command_exists("yt-dlp") {
        if let Ok(output) = Command::new("yt-dlp").arg("--version").output() {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            println!("   yt-dlp: {}", version);
        }
    }

    if command_exists("ffmpeg") {
        if let Ok(output) = Command::new("ffmpeg").arg("-version").output() {
            let version_line = String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or("unknown")
                .to_string();
            println!("   ffmpeg: {}", version_line);
        }
    }

    // 3. Cookies конфигурация
    println!("\n🍪 3. КОНФИГУРАЦИЯ COOKIES:");
    match (get_cookies_file(), get_cookies_browser()) {
        (Some(file), _) => {
            println!("   Тип: Файл");
            println!("   Путь: {}", file);
            if std::path::Path::new(&file).exists() {
                let size = fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
                println!("   Статус: ✅ Существует ({} байт)", size);
            } else {
                println!("   Статус: ❌ ФАЙЛ НЕ НАЙДЕН");
            }
        }
        (None, Some(browser)) => {
            println!("   Тип: Браузер");
            println!("   Браузер: {}", browser);
            println!("   Статус: ⚠️  Требует Full Disk Access на macOS");
        }
        (None, None) => {
            println!("   Статус: ❌ НЕ НАСТРОЕНЫ");
            println!("\n   📖 Инструкция по настройке:");
            println!("      export YTDL_COOKIES_FILE=./youtube_cookies.txt");
            println!("      См. MACOS_COOKIES_FIX.md для подробностей");
        }
    }

    // 4. Переменные окружения
    println!("\n🔧 4. ПЕРЕМЕННЫЕ ОКРУЖЕНИЯ:");
    let env_vars = vec!["YTDL_COOKIES_FILE", "YTDL_COOKIES_BROWSER", "YTDL_BIN"];

    for var in env_vars {
        match env::var(var) {
            Ok(value) => println!("   {}: {}", var, value),
            Err(_) => println!("   {}: (не установлена)", var),
        }
    }

    // 5. Итоговая оценка
    println!("\n📊 5. ИТОГОВАЯ ОЦЕНКА:");
    let all_tools_ok = tools.iter().all(|(_, exists)| *exists);
    let cookies_ok = get_cookies_file().is_some() || get_cookies_browser().is_some();

    if all_tools_ok && cookies_ok {
        println!("   ✅ Система готова к работе!");
    } else {
        println!("   ⚠️  Обнаружены проблемы:");
        if !all_tools_ok {
            println!("      • Не все необходимые инструменты установлены");
        }
        if !cookies_ok {
            println!("      • Cookies не настроены (YouTube видео не будут скачиваться)");
        }
    }

    println!("\n╔════════════════════════════════════════════════════════════════╗");
    println!("║                   ДИАГНОСТИКА ЗАВЕРШЕНА                        ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");
}
