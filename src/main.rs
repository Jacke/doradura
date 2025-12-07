use anyhow::Result;
use dotenvy::dotenv;
use dptree::di::DependencyMap;
use rand::Rng;
use reqwest::ClientBuilder;
use simplelog::*;
use std::fs::read_to_string;
use std::fs::File;
use std::path::Path;
use std::process::Command as ProcessCommand;
use std::sync::Arc;
use std::time::Duration;
use teloxide::dispatching::{Dispatcher, UpdateFilterExt};
use teloxide::prelude::*;
use teloxide::types::{BotCommand, Message, ParseMode};
use teloxide::utils::command::BotCommands;
use tokio::signal;
use tokio::time::{interval, sleep};

// Use library modules
use doradura::core::{
    config, export, history,
    rate_limiter::{self, RateLimiter},
    stats, subscription,
};
use doradura::download::queue::{self as queue};
use doradura::download::ytdlp::{self as ytdlp};
use doradura::download::{
    download_and_send_audio, download_and_send_subtitles, download_and_send_video, DownloadQueue,
};
use doradura::storage::backup::{create_backup, list_backups};
use doradura::storage::db::{
    self as db, create_user, expire_old_subscriptions, get_all_users, get_failed_tasks, get_user,
    log_request, update_user_plan,
};
use doradura::storage::{create_pool, get_connection};
use doradura::telegram::commands::{handle_info_command, handle_message};
use doradura::telegram::menu::{handle_menu_callback, show_main_menu};
use doradura::telegram::notifications::notify_admin_task_failed;
use doradura::telegram::webapp::{run_webapp_server, WebAppAction, WebAppData};
use export::show_export_menu;
use history::show_history;
use stats::{show_global_stats, show_user_stats};
use std::env;
use subscription::show_subscription_info;

#[derive(BotCommands, Clone, Debug)]
#[command(rename_rule = "lowercase", description = "Я умею:")]
enum Command {
    #[command(description = "показывает главное меню")]
    Start,
    #[command(description = "настройки режима загрузки")]
    Mode,
    #[command(description = "показать информацию о доступных форматах")]
    Info,
    #[command(description = "история загрузок")]
    History,
    #[command(description = "личная статистика")]
    Stats,
    #[command(description = "глобальная статистика")]
    Global,
    #[command(description = "экспорт истории")]
    Export,
    #[command(description = "создать бэкап БД (только для администраторов)")]
    Backup,
    #[command(description = "информация о подписке и тарифах")]
    Plan,
    #[command(description = "список всех пользователей (только для администратора)")]
    Users,
    #[command(description = "изменить план пользователя (только для администратора)")]
    Setplan,
    #[command(description = "панель управления пользователями (только для администратора)")]
    Admin,
}

/// Main entry point for the Telegram bot
///
/// Initializes logging, database connection pool, rate limiter, download queue,
/// and starts the Telegram bot dispatcher.
///
/// # Errors
///
/// Логирует конфигурацию cookies при старте приложения
fn log_cookies_configuration() {
    log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    log::info!("🍪 Cookies Configuration Check");
    log::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    // Проверяем файл cookies
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

    // Проверяем браузер cookies
    let browser = config::YTDL_COOKIES_BROWSER.as_str();
    if !browser.is_empty() {
        log::info!("✅ YTDL_COOKIES_BROWSER: {}", browser);
        log::info!("   Will extract cookies from browser");
    } else {
        log::warn!("⚠️  YTDL_COOKIES_BROWSER: not set");
    }

    // Итоговый статус
    if config::YTDL_COOKIES_FILE.is_some()
        && !config::YTDL_COOKIES_FILE.as_ref().unwrap().is_empty()
    {
        let cookies_path =
            if std::path::Path::new(config::YTDL_COOKIES_FILE.as_ref().unwrap()).is_absolute() {
                config::YTDL_COOKIES_FILE.as_ref().unwrap().clone()
            } else {
                shellexpand::tilde(config::YTDL_COOKIES_FILE.as_ref().unwrap()).to_string()
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
        log::error!("  3. Or run: ./run_with_cookies.sh");
        log::error!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    }
}

/// Returns an error if initialization fails (logging, database, bot creation).
#[tokio::main]
async fn main() -> Result<()> {
    // Устанавливаем глобальный обработчик паники для перехвата паник в dispatcher
    // Это позволит нам логировать панику и продолжать работу вместо завершения программы
    std::panic::set_hook(Box::new(|panic_info| {
        log::error!("Panic caught: {:?}", panic_info);
        if let Some(location) = panic_info.location() {
            log::error!(
                "Panic at {}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            );
        }
        if let Some(msg) = panic_info.payload().downcast_ref::<&str>() {
            log::error!("Panic message: {}", msg);
        }
        // Не завершаем программу - позволим основному циклу обработать ошибку
    }));

    // Initialize simplelog for both console and file logging
    let log_file =
        File::create("app.log").map_err(|e| anyhow::anyhow!("Failed to create log file: {}", e))?;

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

    // Load environment variables from .env if present
    let _ = dotenv();

    log::info!("Starting bot...");

    // Log cookies configuration at startup
    log_cookies_configuration();

    // Check and update yt-dlp on startup
    if let Err(e) = ytdlp::check_and_update_ytdlp().await {
        log::warn!("Failed to check/update yt-dlp: {}. Continuing anyway.", e);
    }

    // Check if local Bot API server is configured
    let bot = if let Ok(bot_api_url) = std::env::var("BOT_API_URL") {
        log::info!("Using custom Bot API URL: {}", bot_api_url);
        let url = url::Url::parse(&bot_api_url)
            .map_err(|e| anyhow::anyhow!("Invalid BOT_API_URL: {}", e))?;
        Bot::from_env_with_client(
            ClientBuilder::new()
                .timeout(config::network::timeout())
                .build()?,
        )
        .set_api_url(url)
    } else {
        Bot::from_env_with_client(
            ClientBuilder::new()
                .timeout(config::network::timeout())
                .build()?,
        )
    };

    let mut retry_count = 0;
    let max_retries = config::retry::MAX_DISPATCHER_RETRIES;

    // Get bot information to check mentions
    let bot_info = bot.get_me().await?;
    let bot_username = bot_info.username.as_deref();
    let bot_id = bot_info.id;
    log::info!("Bot username: {:?}, Bot ID: {}", bot_username, bot_id);

    // Set the list of bot commands
    bot.set_my_commands(vec![
        BotCommand::new("start", "показывает главное меню"),
        BotCommand::new("mode", "настройки режима загрузки"),
        BotCommand::new("info", "показать информацию о доступных форматах"),
        BotCommand::new("history", "история загрузок"),
        BotCommand::new("stats", "личная статистика"),
        BotCommand::new("global", "глобальная статистика"),
        BotCommand::new("export", "экспорт истории"),
        BotCommand::new("backup", "создать бэкап БД (только для администраторов)"),
        BotCommand::new("plan", "информация о подписке и тарифах"),
        BotCommand::new(
            "users",
            "список всех пользователей (только для администратора)",
        ),
        BotCommand::new(
            "setplan",
            "изменить план пользователя (только для администратора)",
        ),
    ])
    .await?;

    // Create database connection pool
    let db_pool = Arc::new(
        create_pool("database.sqlite")
            .map_err(|e| anyhow::anyhow!("Failed to create database pool: {}", e))?,
    );

    // Read and apply the migration.sql file
    let migration_sql = read_to_string("migration.sql")?;
    let conn = get_connection(&db_pool)
        .map_err(|e| anyhow::anyhow!("Failed to get database connection: {}", e))?;
    // Execute migration, but don't fail if some steps already exist
    if let Err(e) = conn.execute_batch(&migration_sql) {
        log::warn!(
            "Some migration steps failed (this is normal if tables/columns already exist): {}",
            e
        );
    }

    // Start audio effects cleanup task
    doradura::download::audio_effects::start_cleanup_task(Arc::clone(&db_pool));

    let rate_limiter = Arc::new(RateLimiter::new());
    let download_queue = Arc::new(DownloadQueue::new());

    // Не восстанавливаем failed задачи при запуске - пользователь должен сам повторить запрос
    // recover_failed_tasks(&download_queue, &db_pool).await;

    // Start Mini App web server if WEBAPP_PORT is set
    if let Ok(webapp_port_str) = env::var("WEBAPP_PORT") {
        if let Ok(webapp_port) = webapp_port_str.parse::<u16>() {
            log::info!("Starting Mini App web server on port {}", webapp_port);
            let db_pool_webapp = Arc::clone(&db_pool);
            let download_queue_webapp = Arc::clone(&download_queue);
            let rate_limiter_webapp = Arc::clone(&rate_limiter);
            let bot_token_webapp = bot.token().to_string();

            tokio::spawn(async move {
                if let Err(e) = run_webapp_server(
                    webapp_port,
                    db_pool_webapp,
                    download_queue_webapp,
                    rate_limiter_webapp,
                    bot_token_webapp,
                )
                .await
                {
                    log::error!("Mini App web server error: {}", e);
                }
            });
        } else {
            log::warn!("Invalid WEBAPP_PORT value: {}", webapp_port_str);
        }
    } else {
        log::info!("WEBAPP_PORT not set, Mini App web server disabled");
        log::info!(
            "Set WEBAPP_PORT environment variable to enable Mini App (e.g., WEBAPP_PORT=8080)"
        );
    }

    // Start the queue processing
    tokio::spawn(process_queue(
        bot.clone(),
        Arc::clone(&download_queue),
        Arc::clone(&rate_limiter),
        Arc::clone(&db_pool),
    ));

    // Start automatic backup scheduler (daily backups)
    let db_path = "database.sqlite".to_string();
    tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(24 * 60 * 60)); // 24 hours
        loop {
            interval.tick().await;
            match create_backup(&db_path) {
                Ok(path) => log::info!("Automatic backup created: {}", path.display()),
                Err(e) => log::error!("Failed to create automatic backup: {}", e),
            }
        }
    });

    // Start automatic subscription expiry checker (every hour)
    let db_pool_expiry = Arc::clone(&db_pool);
    tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(60 * 60)); // 1 hour
        loop {
            interval.tick().await;
            match get_connection(&db_pool_expiry) {
                Ok(conn) => {
                    match expire_old_subscriptions(&conn) {
                        Ok(count) if count > 0 => {
                            log::info!("Expired {} subscription(s) automatically", count);
                        }
                        Ok(_) => {} // No expired subscriptions
                        Err(e) => log::error!("Failed to expire old subscriptions: {}", e),
                    }
                }
                Err(e) => log::error!("Failed to get DB connection for expiry check: {}", e),
            }
        }
    });

    // Create a dispatcher to handle both commands and plain messages
    let handler = dptree::entry()
        // Обработчик Web App Data - должен быть ПЕРВЫМ для обработки данных из Mini App
        .branch(
            Update::filter_message()
                .filter(|msg: Message| msg.web_app_data().is_some())
                .endpoint({
                    let download_queue = Arc::clone(&download_queue);
                    let db_pool = Arc::clone(&db_pool);
                    move |bot: Bot, msg: Message| {
                        let download_queue = Arc::clone(&download_queue);
                        let db_pool = Arc::clone(&db_pool);
                        async move {
                            log::info!("Received web_app_data message");

                            if let Some(web_app_data) = msg.web_app_data() {
                                let data_str = &web_app_data.data;
                                log::debug!("Web App Data: {}", data_str);

                                // Создаем пользователя если его нет
                                match get_connection(&db_pool) {
                                    Ok(conn) => {
                                        let chat_id = msg.chat.id.0;
                                        if let Ok(None) = get_user(&conn, chat_id) {
                                            let _ = create_user(&conn, chat_id, msg.from.as_ref().and_then(|u| u.username.clone()));
                                        }
                                    }
                                    Err(e) => log::error!("Failed to get DB connection: {}", e),
                                }

                                // Пытаемся распарсить как новый формат с action
                                if let Ok(action_data) = serde_json::from_str::<WebAppAction>(data_str) {
                                    log::info!("Parsed Web App Action: {:?}", action_data);

                                    match action_data.action.as_str() {
                                        "upgrade_plan" => {
                                            if let Some(plan) = action_data.plan {
                                                let plan_name = match plan.as_str() {
                                                    "premium" => "Premium",
                                                    "vip" => "VIP",
                                                    _ => "неизвестный",
                                                };

                                                let message = format!(
                                                    "🚀 *Подключение тарифа {}*\n\n\
                                                    Для подключения подписки используйте команду /plan и выберите нужный тариф.\n\n\
                                                    Там вы сможете ознакомиться с условиями и оплатить подписку.",
                                                    plan_name
                                                );

                                                let _ = bot.send_message(msg.chat.id, message)
                                                    .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                                                    .await;

                                                log::info!("User {} requested upgrade to {}", msg.chat.id, plan);
                                            }
                                        }
                                        _ => {
                                            log::warn!("Unknown action: {}", action_data.action);
                                        }
                                    }
                                }
                                // Если не получилось как action, пытаемся как старый формат WebAppData
                                else if let Ok(app_data) = serde_json::from_str::<WebAppData>(data_str) {
                                    log::info!("Parsed Web App Data (legacy): {:?}", app_data);

                                    // Парсим URL и добавляем задачу в очередь
                                    match url::Url::parse(&app_data.url) {
                                        Ok(url) => {
                                            let is_video = app_data.format == "mp4";
                                            let format = app_data.format.clone();

                                            let task = queue::DownloadTask::new(
                                                url.to_string(),
                                                msg.chat.id,
                                                Some(msg.id.0),
                                                is_video,
                                                format,
                                                app_data.video_quality,
                                                app_data.audio_bitrate,
                                            );

                                            download_queue.add_task(task, Some(Arc::clone(&db_pool))).await;

                                            let _ = bot.send_message(
                                                msg.chat.id,
                                                "✅ Задача добавлена в очередь! Скоро отправлю файл."
                                            ).await;

                                            log::info!("Task from Mini App added to queue for user {}", msg.chat.id);
                                        }
                                        Err(e) => {
                                            log::error!("Invalid URL from Mini App: {}", e);
                                            let _ = bot.send_message(
                                                msg.chat.id,
                                                "❌ Некорректная ссылка. Попробуй еще раз."
                                            ).await;
                                        }
                                    }
                                } else {
                                    log::error!("Failed to parse Web App Data as any known format");
                                    let _ = bot.send_message(
                                        msg.chat.id,
                                        "❌ Ошибка обработки данных. Попробуй еще раз."
                                    ).await;
                                }
                            }

                            respond(())
                        }
                    }
                })
        )
        // ВАЖНО: Обработчик successful_payment должен быть ВТОРЫМ, до обработки обычных сообщений
        .branch(
            Update::filter_message()
                .filter(|msg: Message| msg.successful_payment().is_some())
                .endpoint({
                    let db_pool = Arc::clone(&db_pool);
                    move |bot: Bot, msg: Message| {
                        let db_pool = Arc::clone(&db_pool);
                        async move {
                            log::info!("Received successful_payment message");
                            // Используем централизованный обработчик платежей с поддержкой рекуррентных подписок
                            if let Err(e) = subscription::handle_successful_payment(&bot, &msg, Arc::clone(&db_pool)).await {
                                log::error!("Failed to handle successful payment: {:?}", e);
                            }
                            respond(())
                        }
                    }
                })
        )
        .branch(Update::filter_message().branch(
            dptree::entry()
                .filter_command::<Command>()
                .endpoint({
                    let db_pool = Arc::clone(&db_pool);
                    move |bot: Bot, msg: Message, cmd: Command| {
                        let db_pool = Arc::clone(&db_pool);
                        async move {
                            log::debug!("Received command: {:?}", cmd);
                            match cmd {
                                Command::Start => {
                                    // Список file_id стикеров из стикерпака doraduradoradura
                                    let sticker_file_ids = vec![
                                        "CAACAgIAAxUAAWj-ZokEQu5YpTnjl6IWPzCQZ0UUAAJCEwAC52QwSC6nTghQdw-KNgQ",
                                        "CAACAgIAAxUAAWj-ZomIQgQKKpbMZA0_VDzfavIiAAK1GgACt8dBSNRj5YvFS-dmNgQ",
                                        "CAACAgIAAxUAAWj-Zokct93wagdDXh1JbhxBIyJOAALzFwACoktASAOjHltqzx0ENgQ",
                                        "CAACAgIAAxUAAWj-ZomorWU-YHGN6oQ6-ikN46CJAAInFAACqlJYSGHilrVqW1AxNgQ",
                                        "CAACAgIAAxUAAWj-ZonVzqfhCC1-YjDNhqGioqvVAALdEwAC-_ZpSB5PRC_sd93QNgQ",
                                        "CAACAgIAAxkBAAIFymj-YswNosbIex7SmXJejbO_GN7-AAJMGQAC9MFQSHBzdKlbjXskNgQ",
                                        "CAACAgIAAxUAAWj-Zol_H6tZIPG-PPHnpNZS1QkIAAJFGwACIQtBSDwm6rS-ZojVNgQ",
                                        "CAACAgIAAxUAAWj-ZomOtDnC9_6jFRp84js-HQN5AALzEgACqc5ISI4uefJ9dzZPNgQ",
                                        "CAACAgIAAxUAAWj-ZolmPZFTqhyNqwssS4JVQY_AAALgFAACU7NBSCIDa2YqXjXyNgQ",
                                        "CAACAgIAAxUAAWj-ZonZTWGW2DadfQ2Mo6bHAAHy2AACjxEAAgSTSUj1H3gU_UUHdjYE",
                                        "CAACAgIAAxUAAWj-ZolQ6OCfECavW19ATgcCup5PAAIOFgACgbdJSMOkkJfpAbs_NgQ",
                                        "CAACAgIAAxUAAWj-Zol19ilXmGth6SKa-4FRrSEJAAJRFwACM9JISKFYdRXvbsb1NgQ",
                                        "CAACAgIAAxUAAWj-ZokRA50GUCiz_OXQUih3uljfAAIeGQACsyBISDP8m_5FL5CJNgQ",
                                        "CAACAgIAAxUAAWj-ZomiM5Mt2aK1G3b8O7JK-shMAALPFQACWGhoSMeITTonc71ENgQ",
                                        "CAACAgIAAxUAAWj-ZomSF9AsKZr6myR3lYgyc-HyAAIRGQACM9KRSG5IUy40KB2KNgQ",
                                    ];

                                    // Генерируем случайный индекс используя настоящий генератор случайных чисел
                                    // Используем rand для лучшего разнообразия (timestamp может быть одинаковым для быстрых отправок)
                                    let random_index = rand::thread_rng().gen_range(0..sticker_file_ids.len());
                                    let random_sticker_id = sticker_file_ids[random_index];

                                    // Отправляем случайный стикер
                                    let _ = bot.send_sticker(msg.chat.id, teloxide::types::InputFile::file_id(teloxide::types::FileId(random_sticker_id.to_string()))).await;

                                    // Отправляем приветственное сообщение
                                    let _ = bot.send_message(msg.chat.id, "Хэй\\! Я Дора, дай мне ссылку и я скачаю ❤️‍🔥")
                                        .parse_mode(ParseMode::MarkdownV2)
                                        .await;

                                    // Отправляем кнопку для открытия Mini App (если WEBAPP_URL настроен)
                                    if let Ok(webapp_url) = env::var("WEBAPP_URL") {
                                        use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, WebAppInfo};

                                        let keyboard = InlineKeyboardMarkup::new(vec![
                                            vec![InlineKeyboardButton::web_app(
                                                "🚀 Открыть Mini App",
                                                WebAppInfo { url: webapp_url.parse().unwrap() }
                                            )],
                                        ]);

                                        let _ = bot.send_message(
                                            msg.chat.id,
                                            "💡 Попробуй новый Mini App для удобного скачивания!"
                                        )
                                        .reply_markup(keyboard)
                                        .await;
                                    }

                                    // Отправка случайного голосового сообщения в случайный момент
                                    let bot_voice = bot.clone();
                                    let chat_id_voice = msg.chat.id;
                                    tokio::spawn(async move {
                                        // Генерируем случайную вероятность отправки (70% шанс)
                                        let should_send = rand::thread_rng().gen_bool(0.7);
                                        if !should_send {
                                            log::debug!("Voice message skipped by random chance for chat {}", chat_id_voice);
                                            return;
                                        }

                                        // Генерируем случайную задержку от 2 до 10 секунд
                                        let delay_secs = rand::thread_rng().gen_range(2000..=10000);

                                        // Ждем случайное время
                                        sleep(Duration::from_millis(delay_secs)).await;

                                        // Находим доступные голосовые файлы
                                        let available_files: Vec<&str> = VOICE_FILES
                                            .iter()
                                            .filter(|&&file| Path::new(file).exists())
                                            .copied()
                                            .collect();

                                        if available_files.is_empty() {
                                            log::warn!("No voice files found from: {:?}, skipping voice message", VOICE_FILES);
                                            return;
                                        }

                                        // Случайно выбираем один из доступных файлов
                                        let selected_file = available_files[rand::thread_rng().gen_range(0..available_files.len())];
                                        log::debug!("Selected voice file: {} for chat {}", selected_file, chat_id_voice);

                                        // Отправляем выбранный голосовой файл с waveform
                                        send_voice_with_waveform(bot_voice, chat_id_voice, selected_file).await;
                                    });
                                }
                                Command::Mode => {
                                    let _ = show_main_menu(&bot, msg.chat.id, db_pool).await;
                                }
                                Command::Info => {
                                    let _ = handle_info_command(bot.clone(), msg.clone()).await;
                                }
                                Command::History => {
                                    let _ = show_history(&bot, msg.chat.id, db_pool).await;
                                }
                                Command::Stats => {
                                    let _ = show_user_stats(&bot, msg.chat.id, db_pool).await;
                                }
                                Command::Global => {
                                    let _ = show_global_stats(&bot, msg.chat.id, db_pool).await;
                                }
                                Command::Export => {
                                    let _ = show_export_menu(&bot, msg.chat.id, db_pool).await;
                                }
                                Command::Backup => {
                                    // Проверяем, является ли пользователь администратором stansob
                                    let is_admin = msg.from.as_ref()
                                        .and_then(|u| u.username.as_ref())
                                        .map(|username| username == "stansob")
                                        .unwrap_or(false);

                                    if is_admin {
                                        match create_backup("database.sqlite") {
                                            Ok(backup_path) => {
                                                let backups = list_backups().unwrap_or_default();
                                                let _ = bot.send_message(
                                                    msg.chat.id,
                                                    format!(
                                                        "✅ Бэкап создан успешно!\n\n📁 Путь: {}\n📊 Всего бэкапов: {}",
                                                        backup_path.display(),
                                                        backups.len()
                                                    )
                                                ).await;
                                            }
                                            Err(e) => {
                                                let _ = bot.send_message(
                                                    msg.chat.id,
                                                    format!("❌ Ошибка при создании бэкапа: {}", e)
                                                ).await;
                                            }
                                        }
                                    } else {
                                        let _ = bot.send_message(
                                            msg.chat.id,
                                            "❌ У тебя нет прав для выполнения этой команды."
                                        ).await;
                                    }
                                }
                                Command::Plan => {
                                    let _ = show_subscription_info(&bot, msg.chat.id, db_pool).await;
                                }
                                Command::Users => {
                                    // Проверяем, является ли пользователь администратором stansob
                                    let username = msg.from.as_ref()
                                        .and_then(|u| u.username.clone());
                                    let is_admin = username.as_ref()
                                        .map(|u| u == "stansob")
                                        .unwrap_or(false);

                                    log::debug!("Users command: username={:?}, is_admin={}", username, is_admin);

                                    if is_admin {
                                        match get_connection(&db_pool) {
                                            Ok(conn) => {
                                                match get_all_users(&conn) {
                                                    Ok(users) => {
                                                        log::debug!("Found {} users in database", users.len());

                                                        if users.is_empty() {
                                                            let _ = bot.send_message(
                                                                msg.chat.id,
                                                                "👥 *Список пользователей*\n\nВ базе данных пока нет пользователей\\."
                                                            )
                                                            .parse_mode(ParseMode::MarkdownV2)
                                                            .await;
                                                        } else {
                                                            // Функция экранирования для MarkdownV2
                                                            fn escape_markdown(text: &str) -> String {
                                                                let mut result = String::with_capacity(text.len() * 2);

                                                                for c in text.chars() {
                                                                    match c {
                                                                        '\\' => result.push_str("\\\\"),
                                                                        '_' => result.push_str("\\_"),
                                                                        '*' => result.push_str("\\*"),
                                                                        '[' => result.push_str("\\["),
                                                                        ']' => result.push_str("\\]"),
                                                                        '(' => result.push_str("\\("),
                                                                        ')' => result.push_str("\\)"),
                                                                        '~' => result.push_str("\\~"),
                                                                        '`' => result.push_str("\\`"),
                                                                        '>' => result.push_str("\\>"),
                                                                        '#' => result.push_str("\\#"),
                                                                        '+' => result.push_str("\\+"),
                                                                        '-' => result.push_str("\\-"),
                                                                        '=' => result.push_str("\\="),
                                                                        '|' => result.push_str("\\|"),
                                                                        '{' => result.push_str("\\{"),
                                                                        '}' => result.push_str("\\}"),
                                                                        '.' => result.push_str("\\."),
                                                                        '!' => result.push_str("\\!"),
                                                                        _ => result.push(c),
                                                                    }
                                                                }

                                                                result
                                                            }

                                                            const MAX_MESSAGE_LENGTH: usize = 4000; // Telegram limit is 4096, leave some margin

                                                            // Подсчет статистики
                                                            let free_count = users.iter().filter(|u| u.plan == "free").count();
                                                            let premium_count = users.iter().filter(|u| u.plan == "premium").count();
                                                            let vip_count = users.iter().filter(|u| u.plan == "vip").count();
                                                            let with_subscription = users.iter().filter(|u| u.telegram_charge_id.is_some()).count();

                                                            let total_users = escape_markdown(&users.len().to_string());
                                                            let free_escaped = escape_markdown(&free_count.to_string());
                                                            let premium_escaped = escape_markdown(&premium_count.to_string());
                                                            let vip_escaped = escape_markdown(&vip_count.to_string());
                                                            let subs_escaped = escape_markdown(&with_subscription.to_string());

                                                            let mut text = format!(
                                                                "👥 *Список пользователей* \\(всего\\: {}\\)\n\n\
                                                                📊 Статистика:\n\
                                                                • 🌟 Free: {}\n\
                                                                • ⭐ Premium: {}\n\
                                                                • 👑 VIP: {}\n\
                                                                • 💫 Активных подписок: {}\n\n\
                                                                ━━━━━━━━━━━━━━━━━━━━\n\n",
                                                                total_users, free_escaped, premium_escaped, vip_escaped, subs_escaped
                                                            );
                                                            let mut users_added = 0;

                                                            for (idx, user) in users.iter().enumerate() {
                                                                let username_str = user.username.as_ref()
                                                                    .map(|u| {
                                                                        let escaped = escape_markdown(u);
                                                                        format!("@{}", escaped)
                                                                    })
                                                                    .unwrap_or_else(|| {
                                                                        let id_escaped = escape_markdown(&user.telegram_id.to_string());
                                                                        format!("ID\\: {}", id_escaped)
                                                                    });
                                                                let plan_emoji = match user.plan.as_str() {
                                                                    "premium" => "⭐",
                                                                    "vip" => "👑",
                                                                    _ => "🌟",
                                                                };

                                                                // Показываем иконку подписки если есть
                                                                let sub_icon = if user.telegram_charge_id.is_some() {
                                                                    " 💫"
                                                                } else {
                                                                    ""
                                                                };

                                                                let plan_escaped = escape_markdown(&user.plan);
                                                                let idx_escaped = escape_markdown(&(idx + 1).to_string());
                                                                let user_line = format!(
                                                                    "{}\\. {} {} {}{}\n",
                                                                    idx_escaped,
                                                                    username_str,
                                                                    plan_emoji,
                                                                    plan_escaped,
                                                                    sub_icon
                                                                );

                                                                // Проверяем, не превысит ли добавление этой строки лимит
                                                                if text.len() + user_line.len() > MAX_MESSAGE_LENGTH {
                                                                    let remaining = escape_markdown(&(users.len() - users_added).to_string());
                                                                    text.push_str(&format!("\n\\.\\.\\. и еще {} пользователей", remaining));
                                                                    break;
                                                                }

                                                                text.push_str(&user_line);
                                                                users_added += 1;
                                                            }

                                                            log::debug!("Sending users list with {} users (text length: {})", users_added, text.len());

                                                            match bot.send_message(msg.chat.id, &text)
                                                                .parse_mode(ParseMode::MarkdownV2)
                                                                .await
                                                            {
                                                                Ok(_) => {
                                                                    log::debug!("Successfully sent users list");
                                                                }
                                                                Err(e) => {
                                                                    log::error!("Failed to send users list: {:?}", e);
                                                                    // Попробуем отправить без Markdown, если была ошибка форматирования
                                                                    let text_plain = text.replace("\\", "").replace("*", "");
                                                                    let _ = bot.send_message(
                                                                        msg.chat.id,
                                                                        format!("❌ Ошибка при отправке списка. Попробую без форматирования:\n\n{}", text_plain)
                                                                    ).await;
                                                                }
                                                            }
                                                        }
                                                    }
                                                    Err(e) => {
                                                        log::error!("Failed to get users from database: {}", e);
                                                        let _ = bot.send_message(
                                                            msg.chat.id,
                                                            format!("❌ Ошибка при получении списка пользователей: {}", e)
                                                        ).await;
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                log::error!("Failed to get database connection: {}", e);
                                                let _ = bot.send_message(
                                                    msg.chat.id,
                                                    format!("❌ Ошибка подключения к БД: {}", e)
                                                ).await;
                                            }
                                        }
                                    } else {
                                        log::warn!("User {:?} tried to access /users command without permission", username);
                                        let _ = bot.send_message(
                                            msg.chat.id,
                                            "❌ У тебя нет прав для выполнения этой команды."
                                        ).await;
                                    }
                                }
                                Command::Setplan => {
                                    // Проверяем, является ли пользователь администратором stansob
                                    let is_admin = msg.from.as_ref()
                                        .and_then(|u| u.username.as_ref())
                                        .map(|username| username == "stansob")
                                        .unwrap_or(false);

                                    if is_admin {
                                        // Формат команды: /setplan <user_id> <plan>
                                        if let Some(text) = msg.text() {
                                            let parts: Vec<&str> = text.split_whitespace().collect();
                                            if parts.len() == 3 {
                                                match parts[1].parse::<i64>() {
                                                    Ok(user_id) => {
                                                        let plan = parts[2];
                                                        if ["free", "premium", "vip"].contains(&plan) {
                                                            match get_connection(&db_pool) {
                                                                Ok(conn) => {
                                                                    match update_user_plan(&conn, user_id, plan) {
                                                                        Ok(_) => {
                                                                            let plan_emoji = match plan {
                                                                                "premium" => "⭐",
                                                                                "vip" => "👑",
                                                                                _ => "🌟",
                                                                            };
                                                                            let plan_name = match plan {
                                                                                "premium" => "Premium",
                                                                                "vip" => "VIP",
                                                                                _ => "Free",
                                                                            };

                                                                            // Отправляем сообщение администратору
                                                                            let _ = bot.send_message(
                                                                                msg.chat.id,
                                                                                format!("✅ План пользователя {} изменен на {} {}", user_id, plan_emoji, plan)
                                                                            ).await;

                                                                            // Отправляем уведомление пользователю, чей план был изменен
                                                                            let user_chat_id = teloxide::types::ChatId(user_id);
                                                                            let _ = bot.send_message(
                                                                                user_chat_id,
                                                                                format!(
                                                                                    "💳 *Изменение плана подписки*\n\n\
                                                                                    Твой план был изменен администратором\\.\n\n\
                                                                                    *Новый план:* {} {}\n\n\
                                                                                    Изменения вступят в силу немедленно\\! 🎉",
                                                                                    plan_emoji,
                                                                                    plan_name
                                                                                )
                                                                            )
                                                                            .parse_mode(ParseMode::MarkdownV2)
                                                                            .await;
                                                                        }
                                                                        Err(e) => {
                                                                            let _ = bot.send_message(
                                                                                msg.chat.id,
                                                                                format!("❌ Ошибка при обновлении плана: {}", e)
                                                                            ).await;
                                                                        }
                                                                    }
                                                                }
                                                                Err(e) => {
                                                                    let _ = bot.send_message(
                                                                        msg.chat.id,
                                                                        format!("❌ Ошибка подключения к БД: {}", e)
                                                                    ).await;
                                                                }
                                                            }
                                                        } else {
                                                            let _ = bot.send_message(
                                                                msg.chat.id,
                                                                "❌ Неверный план. Используй: free, premium или vip"
                                                            ).await;
                                                        }
                                                    }
                                                    Err(_) => {
                                                        let _ = bot.send_message(
                                                            msg.chat.id,
                                                            "❌ Неверный формат user_id. Используй: /setplan <user_id> <plan>"
                                                        ).await;
                                                    }
                                                }
                                            } else {
                                                let _ = bot.send_message(
                                                    msg.chat.id,
                                                    "❌ Неверный формат команды. Используй: /setplan <user_id> <plan>\nПример: /setplan 123456789 premium"
                                                ).await;
                                            }
                                        }
                                    } else {
                                        let _ = bot.send_message(
                                            msg.chat.id,
                                            "❌ У тебя нет прав для выполнения этой команды."
                                        ).await;
                                    }
                                }
                                Command::Admin => {
                                    // Проверяем, является ли пользователь администратором stansob
                                    let is_admin = msg.from.as_ref()
                                        .and_then(|u| u.username.as_ref())
                                        .map(|username| username == "stansob")
                                        .unwrap_or(false);

                                    if is_admin {
                                        // Показываем панель управления
                                        match get_connection(&db_pool) {
                                            Ok(conn) => {
                                                match get_all_users(&conn) {
                                                    Ok(users) => {
                                                        use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

                                                        // Создаем inline клавиатуру с пользователями (по 2 в ряд)
                                                        let mut keyboard_rows = Vec::new();
                                                        let mut current_row = Vec::new();

                                                        for user in users.iter().take(20) { // Показываем первых 20 пользователей
                                                            let username_display = user.username.as_ref()
                                                                .map(|u| format!("@{}", u))
                                                                .unwrap_or_else(|| format!("ID:{}", user.telegram_id));

                                                            let plan_emoji = match user.plan.as_str() {
                                                                "premium" => "⭐",
                                                                "vip" => "👑",
                                                                _ => "🌟",
                                                            };

                                                            let button_text = format!("{} {}", plan_emoji, username_display);
                                                            let callback_data = format!("admin:user:{}", user.telegram_id);

                                                            current_row.push(InlineKeyboardButton::callback(
                                                                button_text,
                                                                callback_data
                                                            ));

                                                            // Каждые 2 кнопки создаём новый ряд
                                                            if current_row.len() == 2 {
                                                                keyboard_rows.push(current_row.clone());
                                                                current_row.clear();
                                                            }
                                                        }

                                                        // Добавляем оставшиеся кнопки если есть
                                                        if !current_row.is_empty() {
                                                            keyboard_rows.push(current_row);
                                                        }

                                                        let keyboard = InlineKeyboardMarkup::new(keyboard_rows);

                                                        let _ = bot.send_message(
                                                            msg.chat.id,
                                                            format!(
                                                                "🔧 *Панель управления пользователями*\n\n\
                                                                Выбери пользователя для управления:\n\n\
                                                                Показано: {} из {}\n\n\
                                                                💡 Для управления конкретным пользователем используй:\n\
                                                                `/setplan <user_id> <plan>`",
                                                                users.len().min(20),
                                                                users.len()
                                                            )
                                                        )
                                                        .parse_mode(ParseMode::MarkdownV2)
                                                        .reply_markup(keyboard)
                                                        .await;
                                                    }
                                                    Err(e) => {
                                                        let _ = bot.send_message(
                                                            msg.chat.id,
                                                            format!("❌ Ошибка при получении списка пользователей: {}", e)
                                                        ).await;
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                let _ = bot.send_message(
                                                    msg.chat.id,
                                                    format!("❌ Ошибка подключения к БД: {}", e)
                                                ).await;
                                            }
                                        }
                                    } else {
                                        let _ = bot.send_message(
                                            msg.chat.id,
                                            "❌ У тебя нет прав для выполнения этой команды."
                                        ).await;
                                    }
                                }
                            }
                            respond(())
                        }
                    }
                })
        ))
        .branch(
            Update::filter_message()
                .filter({
                    let bot_username = bot_username.map(|s| s.to_string());
                    let bot_id_clone = bot_id;
                    move |msg: Message| {
                        is_message_addressed_to_bot(&msg, bot_username.as_deref(), bot_id_clone)
                    }
                })
                .endpoint({
            let rate_limiter = Arc::clone(&rate_limiter);
            let download_queue = Arc::clone(&download_queue);
            let db_pool = Arc::clone(&db_pool);
            move |bot: Bot, msg: Message| {
                let rate_limiter = Arc::clone(&rate_limiter);
                let download_queue = Arc::clone(&download_queue);
                let db_pool = Arc::clone(&db_pool);
                async move {
                    // Handle message and get user info (optimized: avoids duplicate DB query)
                    let user_info_result = handle_message(bot.clone(), msg.clone(), download_queue.clone(), rate_limiter.clone(), db_pool.clone()).await;

                    // Log request and manage user (reuse user info if available)
                    if let Some(text) = msg.text() {
                        match &user_info_result {
                            Ok(Some(user)) => {
                                // User info already retrieved in handle_message, reuse it
                                match get_connection(&db_pool) {
                                    Ok(conn) => {
                                        if let Err(e) = log_request(&conn, user.telegram_id(), text) {
                                            log::error!("Failed to log request: {}", e);
                                        }
                                    }
                                    Err(e) => {
                                        log::error!("Failed to get database connection: {}", e);
                                    }
                                }
                            }
                            Ok(None) | Err(_) => {
                                // User not found or error occurred, try to get/create user
                                match get_connection(&db_pool) {
                                    Ok(conn) => {
                                        let chat_id = msg.chat.id.0;
                                        match get_user(&conn, chat_id) {
                                            Ok(Some(user)) => {
                                                if let Err(e) = log_request(&conn, user.telegram_id(), text) {
                                                    log::error!("Failed to log request: {}", e);
                                                }
                                            }
                                            Ok(None) => {
                                                if let Err(e) = create_user(&conn, chat_id, msg.from.as_ref().and_then(|u| u.username.clone())) {
                                                    log::error!("Failed to create user: {}", e);
                                                } else if let Err(e) = log_request(&conn, chat_id, text) {
                                                    log::error!("Failed to log request for new user: {}", e);
                                                }
                                            }
                                            Err(e) => {
                                                log::error!("Failed to get user from database: {}", e);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        log::error!("Failed to get database connection: {}", e);
                                    }
                                }
                            }
                        }
                    }

                    if let Err(err) = user_info_result {
                        log::error!("Error handling message: {:?}", err);
                    }

                    respond(())
                }
            }
        }))
        .branch(
            Update::filter_pre_checkout_query().endpoint({
                move |bot: Bot, query: teloxide::types::PreCheckoutQuery| async move {
                    let query_id = query.id;
                    let payload = query.invoice_payload;

                    log::info!("Received pre_checkout_query: id={}, payload={}", query_id, payload);

                    // Проверяем payload
                    if payload.starts_with("subscription:") {
                        // Одобряем платеж
                        match bot.answer_pre_checkout_query(query_id.clone(), true).await {
                            Ok(_) => {
                                log::info!("✅ Pre-checkout query approved for payload: {}", payload);
                            }
                            Err(e) => {
                                log::error!("Failed to answer pre_checkout_query: {:?}", e);
                            }
                        }
                    } else {
                        // Отклоняем неизвестный платеж
                        match bot.answer_pre_checkout_query(query_id.clone(), false)
                            .error_message("Неизвестный тип платежа")
                            .await {
                            Ok(_) => {
                                log::info!("Pre-checkout query rejected for payload: {}", payload);
                            }
                            Err(e) => {
                                log::error!("Failed to answer pre_checkout_query: {:?}", e);
                            }
                        }
                    }
                    respond(())
                }
            })
        )
        .branch(Update::filter_callback_query().endpoint({
            let db_pool = Arc::clone(&db_pool);
            let download_queue = Arc::clone(&download_queue);
            let rate_limiter = Arc::clone(&rate_limiter);
            move |bot: Bot, q: CallbackQuery| {
                let db_pool = Arc::clone(&db_pool);
                let download_queue = Arc::clone(&download_queue);
                let rate_limiter = Arc::clone(&rate_limiter);
                async move {
                    handle_menu_callback(bot, q, db_pool, download_queue, rate_limiter).await
                }
            }
        }));

    // Check if webhook mode is enabled
    let webhook_url = env::var("WEBHOOK_URL").ok();

    if let Some(url) = webhook_url {
        // Webhook mode
        log::info!("Starting bot in webhook mode at {}", url);

        // Delete existing webhook to ensure clean state
        let _ = bot.delete_webhook().await;

        // Set webhook
        bot.set_webhook(url::Url::parse(&url)?).await?;
        log::info!("Webhook set successfully");

        // Note: For full webhook support, you need to set up an HTTP server
        // (e.g., using axum) to receive webhook updates from Telegram.
        // For now, webhook URL is set but you need to handle incoming updates
        // via your HTTP server endpoint.
        // This is a placeholder - full implementation requires HTTP server setup.
        log::warn!(
            "Webhook URL set to {}, but HTTP server is not implemented yet.",
            url
        );
        log::warn!("Please set up an HTTP server to receive webhook updates, or use polling mode.");

        // Keep the main thread alive
        tokio::select! {
            _ = signal::ctrl_c() => {
                log::info!("Shutting down gracefully...");
                bot.delete_webhook().await?;
            },
        }
    } else {
        // Long polling mode (default)
        log::info!("Starting bot in long polling mode");

        // Run the dispatcher with retry logic
        loop {
            let bot_clone = bot.clone();
            let handler_clone = handler.clone();

            // Создаем новый dispatcher в отдельной задаче для изоляции паники
            // Паника "TX is dead" будет перехвачена через JoinHandle
            let handle = tokio::spawn(async move {
                Dispatcher::builder(bot_clone, handler_clone)
                    .dependencies(DependencyMap::new())
                    .build()
                    .dispatch()
                    .await
            });

            match handle.await {
                Ok(()) => {
                    // Dispatcher завершился нормально
                    log::info!("Dispatcher shutdown gracefully");
                    break;
                }
                Err(join_err) => {
                    // Задача была отменена или паника
                    if join_err.is_panic() {
                        let panic_msg = join_err.to_string();
                        log::error!("Dispatcher panicked: {}", panic_msg);

                        if panic_msg.contains("TX is dead") || panic_msg.contains("SendError") {
                            log::warn!("Detected TX is dead panic - will reconnect...");
                        }

                        if retry_count < max_retries {
                            retry_count += 1;
                            log::info!(
                                "Retrying dispatcher connection after panic (attempt {}/{})...",
                                retry_count,
                                max_retries
                            );
                            exponential_backoff(retry_count).await;
                        } else {
                            log::error!("Max retries reached after panic. Exiting...");
                            break;
                        }
                    } else {
                        log::warn!("Dispatcher task was cancelled: {}", join_err);
                        break;
                    }
                }
            }

            // Add a delay between retries to avoid overwhelming the API
            if retry_count > 0 {
                sleep(config::retry::dispatcher_delay()).await;
            }
        }
    }

    tokio::select! {
        _ = signal::ctrl_c() => {
            log::info!("Shutting down gracefully...");
        },
    }

    Ok(())
}

/// Проверяет, адресовано ли сообщение боту
///
/// # Параметры
/// - `msg`: сообщение для проверки
/// - `bot_username`: username бота (без @)
/// - `bot_id`: ID бота
///
/// # Возвращает
/// - `true` если сообщение адресовано боту (личный чат, упоминание бота, ответ на сообщение бота)
/// - `false` если сообщение не адресовано боту
fn is_message_addressed_to_bot(
    msg: &Message,
    bot_username: Option<&str>,
    bot_id: teloxide::types::UserId,
) -> bool {
    use teloxide::types::ChatKind;

    // В личных чатах все сообщения адресованы боту
    if matches!(msg.chat.kind, ChatKind::Private(_)) {
        return true;
    }

    // Проверяем, является ли сообщение ответом на сообщение бота
    if let Some(reply_to) = msg.reply_to_message() {
        if let Some(from) = &reply_to.from {
            if from.id == bot_id {
                return true;
            }
        }
    }

    // Проверяем текст сообщения на упоминание бота
    if let Some(text) = msg.text() {
        // Проверяем entities на упоминания
        if let Some(entities) = msg.entities() {
            for entity in entities {
                use teloxide::types::MessageEntityKind;
                if matches!(entity.kind, MessageEntityKind::Mention) {
                    // Извлекаем упоминание из текста
                    let mention = &text[entity.offset..entity.offset + entity.length];
                    // Убираем @ для сравнения
                    let mention_username = mention.strip_prefix('@').unwrap_or(mention);
                    if let Some(username) = bot_username {
                        if mention_username.eq_ignore_ascii_case(username) {
                            return true;
                        }
                    }
                }
            }
        }

        // Проверяем, начинается ли текст с упоминания бота
        if let Some(username) = bot_username {
            let mention_pattern = format!("@{}", username);
            if text.starts_with(&mention_pattern) || text.contains(&mention_pattern) {
                return true;
            }
        }
    }

    false
}

async fn exponential_backoff(retry_count: u32) {
    let delay = Duration::from_secs(config::retry::EXPONENTIAL_BACKOFF_BASE.pow(retry_count));
    tokio::time::sleep(delay).await;
}

/// Список голосовых файлов для случайной отправки при /start
///
/// Чтобы добавить новый файл, просто добавьте его имя в этот вектор
const VOICE_FILES: &[&str] = &[
    "assets/voices/first.wav",
    "assets/voices/second.wav",
    "assets/voices/third.wav",
    "assets/voices/fourth.wav",
];

/// Конвертирует WAV файл в OGG Opus для корректного отображения waveform в Telegram
///
/// # Параметры
/// - `input_path`: путь к исходному WAV файлу
/// - `output_path`: путь для сохранения сконвертированного OGG файла
///
/// # Возвращает
/// - `Ok(duration)` - успешная конвертация, возвращает длительность в секундах
/// - `Err(error)` - ошибка конвертации
fn convert_wav_to_ogg_opus(input_path: &str, output_path: &str) -> Result<Option<u32>> {
    // Проверяем наличие ffmpeg
    let ffmpeg_check = ProcessCommand::new("ffmpeg").arg("-version").output();

    if ffmpeg_check.is_err() {
        return Err(anyhow::anyhow!(
            "ffmpeg not found. Please install ffmpeg to convert voice messages."
        ));
    }

    // Конвертируем WAV в OGG Opus
    let output = ProcessCommand::new("ffmpeg")
        .arg("-i")
        .arg(input_path)
        .arg("-c:a")
        .arg("libopus")
        .arg("-b:a")
        .arg("64k")
        .arg("-application")
        .arg("voip") // Важно для voice messages
        .arg("-y") // Перезаписать выходной файл если существует
        .arg(output_path)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("ffmpeg conversion failed: {}", stderr));
    }

    // Получаем длительность аудио для корректного отображения
    let probe_output = ProcessCommand::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(output_path)
        .output()?;

    let duration = if probe_output.status.success() {
        let duration_str = String::from_utf8_lossy(&probe_output.stdout);
        duration_str.trim().parse::<f64>().ok().map(|d| d as u32)
    } else {
        None
    };

    Ok(duration)
}

/// Отправляет голосовое сообщение с waveform
///
/// # Параметры
/// - `bot`: экземпляр бота для отправки
/// - `chat_id`: ID чата для отправки
/// - `voice_file_path`: путь к WAV файлу
///
/// Конвертирует WAV в OGG Opus и отправляет с указанием duration для waveform
async fn send_voice_with_waveform(
    bot: Bot,
    chat_id: teloxide::types::ChatId,
    voice_file_path: &str,
) {
    if !Path::new(voice_file_path).exists() {
        log::warn!(
            "Voice file {} not found, skipping voice message",
            voice_file_path
        );
        return;
    }

    // Генерируем уникальное имя для временного OGG файла
    let file_stem = Path::new(voice_file_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("voice");
    let ogg_path = format!("{}.ogg", file_stem);

    // Конвертируем WAV в OGG Opus для корректного отображения waveform
    let voice_file_path_clone = voice_file_path.to_string();
    let ogg_path_clone = ogg_path.clone();
    let conversion_result = tokio::task::spawn_blocking(move || {
        convert_wav_to_ogg_opus(&voice_file_path_clone, &ogg_path_clone)
    })
    .await;

    match conversion_result {
        Ok(Ok(duration)) => {
            // Отправляем голосовое сообщение с указанием duration
            let mut voice_msg =
                bot.send_voice(chat_id, teloxide::types::InputFile::file(&ogg_path));

            // Указываем duration для корректного отображения waveform
            if let Some(dur) = duration {
                voice_msg = voice_msg.duration(dur);
            }

            match voice_msg.await {
                Ok(_) => {
                    log::info!(
                        "Voice message {} sent successfully to chat {} (duration: {:?}s)",
                        voice_file_path,
                        chat_id,
                        duration
                    );
                }
                Err(e) => {
                    log::warn!(
                        "Failed to send voice message {} to chat {}: {}",
                        voice_file_path,
                        chat_id,
                        e
                    );
                }
            }

            // Удаляем временный OGG файл
            if let Err(e) = std::fs::remove_file(&ogg_path) {
                log::warn!("Failed to remove temporary OGG file {}: {}", ogg_path, e);
            }
        }
        Ok(Err(e)) => {
            log::warn!(
                "Failed to convert {} to OGG Opus: {}. Trying to send as WAV without waveform.",
                voice_file_path,
                e
            );
            // Fallback: пробуем отправить как WAV (без waveform)
            match bot
                .send_voice(chat_id, teloxide::types::InputFile::file(voice_file_path))
                .await
            {
                Ok(_) => {
                    log::info!(
                        "Voice message {} sent as WAV (no waveform) to chat {}",
                        voice_file_path,
                        chat_id
                    );
                }
                Err(e) => {
                    log::warn!(
                        "Failed to send voice message {} to chat {}: {}",
                        voice_file_path,
                        chat_id,
                        e
                    );
                }
            }
        }
        Err(e) => {
            log::warn!(
                "Failed to spawn conversion task for {}: {}",
                voice_file_path,
                e
            );
        }
    }
}

/// Восстанавливает failed задачи из БД и добавляет их обратно в очередь
#[allow(dead_code)]
async fn recover_failed_tasks(queue: &Arc<DownloadQueue>, db_pool: &Arc<db::DbPool>) {
    match get_connection(db_pool) {
        Ok(conn) => {
            match get_failed_tasks(&conn, config::admin::MAX_TASK_RETRIES) {
                Ok(failed_tasks) => {
                    if failed_tasks.is_empty() {
                        log::info!(
                            "✅ No failed tasks to recover - all tasks are completed or processing"
                        );
                        return;
                    }

                    let task_count = failed_tasks.len();
                    log::info!("═══════════════════════════════════════════════════════════");
                    log::info!("🔄 Found {} failed task(s) in database", task_count);
                    log::info!("═══════════════════════════════════════════════════════════");

                    // Логируем детальную информацию о каждой failed задаче
                    for (idx, task_entry) in failed_tasks.iter().enumerate() {
                        let priority_str = match task_entry.priority {
                            2 => "HIGH",
                            1 => "MEDIUM",
                            _ => "LOW",
                        };

                        let error_preview = task_entry
                            .error_message
                            .as_ref()
                            .map(|e| {
                                let preview = if e.len() > 100 {
                                    format!("{}...", &e[..100])
                                } else {
                                    e.clone()
                                };
                                preview.replace(['\n', '\r'], " ")
                            })
                            .unwrap_or_else(|| "No error message".to_string());

                        log::info!("  [{}/{}] Task ID: {}", idx + 1, task_count, task_entry.id);
                        log::info!("      └─ User ID: {}", task_entry.user_id);
                        log::info!("      └─ URL: {}", task_entry.url);
                        log::info!(
                            "      └─ Format: {} (video: {})",
                            task_entry.format,
                            task_entry.is_video
                        );
                        log::info!("      └─ Priority: {}", priority_str);
                        log::info!(
                            "      └─ Retry count: {}/{}",
                            task_entry.retry_count,
                            config::admin::MAX_TASK_RETRIES
                        );
                        log::info!("      └─ Created: {}", task_entry.created_at);
                        log::info!("      └─ Error: {}", error_preview);
                        log::info!("");
                    }

                    log::info!("═══════════════════════════════════════════════════════════");
                    log::info!("🔄 Starting recovery of {} failed task(s)...", task_count);
                    log::info!("═══════════════════════════════════════════════════════════");

                    let mut recovered_count = 0;

                    for task_entry in failed_tasks {
                        // Конвертируем TaskQueueEntry в DownloadTask
                        let priority = match task_entry.priority {
                            2 => queue::TaskPriority::High,
                            1 => queue::TaskPriority::Medium,
                            _ => queue::TaskPriority::Low,
                        };

                        let download_task = queue::DownloadTask {
                            id: task_entry.id.clone(),
                            url: task_entry.url.clone(),
                            chat_id: teloxide::types::ChatId(task_entry.user_id),
                            message_id: None, // Recovered tasks don't have original message
                            is_video: task_entry.is_video,
                            format: task_entry.format.clone(),
                            video_quality: task_entry.video_quality.clone(),
                            audio_bitrate: task_entry.audio_bitrate.clone(),
                            created_timestamp: chrono::DateTime::parse_from_rfc3339(
                                &task_entry.created_at,
                            )
                            .map(|dt| dt.with_timezone(&chrono::Utc))
                            .unwrap_or_else(|_| chrono::Utc::now()),
                            priority,
                        };

                        // Добавляем задачу обратно в очередь
                        queue
                            .add_task(download_task, Some(Arc::clone(db_pool)))
                            .await;
                        recovered_count += 1;
                        log::info!(
                            "  ✅ Recovered task {} (retry: {}/{}) - URL: {}",
                            task_entry.id,
                            task_entry.retry_count + 1,
                            config::admin::MAX_TASK_RETRIES,
                            task_entry.url
                        );
                    }

                    log::info!("═══════════════════════════════════════════════════════════");
                    log::info!("✅ Recovery completed:");
                    log::info!("   • Found in DB: {} task(s)", task_count);
                    log::info!("   • Successfully recovered: {} task(s)", recovered_count);
                    log::info!("═══════════════════════════════════════════════════════════");
                }
                Err(e) => {
                    log::error!("❌ Failed to get failed tasks from database: {}", e);
                }
            }
        }
        Err(e) => {
            log::error!("❌ Failed to get DB connection for task recovery: {}", e);
        }
    }
}

async fn process_queue(
    bot: Bot,
    queue: Arc<DownloadQueue>,
    rate_limiter: Arc<rate_limiter::RateLimiter>,
    db_pool: Arc<db::DbPool>,
) {
    // Semaphore to limit concurrent downloads
    let semaphore = Arc::new(tokio::sync::Semaphore::new(
        config::queue::MAX_CONCURRENT_DOWNLOADS,
    ));
    let mut interval = interval(config::queue::check_interval());

    loop {
        interval.tick().await;
        if let Some(task) = queue.get_task().await {
            log::info!("Got task {} from queue", task.id);
            let bot = bot.clone();
            let rate_limiter = Arc::clone(&rate_limiter);
            let semaphore = Arc::clone(&semaphore);
            let db_pool = Arc::clone(&db_pool);

            tokio::spawn(async move {
                // Acquire permit from semaphore (will wait if all permits are taken)
                let _permit = match semaphore.acquire().await {
                    Ok(p) => p,
                    Err(e) => {
                        log::error!(
                            "Failed to acquire semaphore permit for task {}: {}",
                            task.id,
                            e
                        );
                        // Помечаем задачу как failed
                        if let Ok(conn) = db::get_connection(&db_pool) {
                            let _ = db::mark_task_failed(
                                &conn,
                                &task.id,
                                &format!("Failed to acquire semaphore: {}", e),
                            );
                        }
                        return;
                    }
                };
                log::info!(
                    "Processing task {} (permits available: {})",
                    task.id,
                    semaphore.available_permits()
                );

                // Помечаем задачу как processing
                if let Ok(conn) = db::get_connection(&db_pool) {
                    if let Err(e) = db::mark_task_processing(&conn, &task.id) {
                        log::warn!("Failed to mark task {} as processing: {}", task.id, e);
                    }
                }

                let url = match url::Url::parse(&task.url) {
                    Ok(u) => u,
                    Err(e) => {
                        log::error!("Invalid URL for task {}: {} - {}", task.id, task.url, e);
                        let error_msg = format!("Invalid URL: {}", e);
                        // Помечаем задачу как failed
                        if let Ok(conn) = db::get_connection(&db_pool) {
                            let _ = db::mark_task_failed(&conn, &task.id, &error_msg);
                            // Уведомляем администратора
                            notify_admin_task_failed(
                                bot.clone(),
                                Arc::clone(&db_pool),
                                &task.id,
                                task.chat_id.0,
                                &task.url,
                                &error_msg,
                            )
                            .await;
                        }
                        return;
                    }
                };

                // Process task based on format
                let db_pool_clone = Arc::clone(&db_pool);
                let video_quality = task.video_quality.clone();
                let audio_bitrate = task.audio_bitrate.clone();
                let task_id = task.id.clone();
                let task_url = task.url.clone();
                let task_format = task.format.clone();
                let task_chat_id = task.chat_id;
                let result = match task.format.as_str() {
                    "mp4" => {
                        download_and_send_video(
                            bot.clone(),
                            task.chat_id,
                            url,
                            rate_limiter.clone(),
                            task.created_timestamp,
                            Some(db_pool_clone.clone()),
                            video_quality,
                            task.message_id,
                        )
                        .await
                    }
                    "srt" | "txt" => {
                        download_and_send_subtitles(
                            bot.clone(),
                            task.chat_id,
                            url,
                            rate_limiter.clone(),
                            task.created_timestamp,
                            task.format.clone(),
                            Some(db_pool_clone.clone()),
                            task.message_id,
                        )
                        .await
                    }
                    _ => {
                        // Default to audio (mp3)
                        download_and_send_audio(
                            bot.clone(),
                            task.chat_id,
                            url,
                            rate_limiter.clone(),
                            task.created_timestamp,
                            Some(db_pool_clone.clone()),
                            audio_bitrate,
                            task.message_id,
                        )
                        .await
                    }
                };

                match result {
                    Ok(_) => {
                        // Помечаем задачу как completed
                        if let Ok(conn) = db::get_connection(&db_pool) {
                            if let Err(e) = db::mark_task_completed(&conn, &task_id) {
                                log::warn!("Failed to mark task {} as completed: {}", task_id, e);
                            }
                        }
                        log::info!("Task {} completed successfully", task_id);
                    }
                    Err(e) => {
                        let error_msg = format!("{:?}", e);
                        log::error!(
                            "Failed to process task {} (format: {}): {}",
                            task_id,
                            task_format,
                            error_msg
                        );

                        // Помечаем задачу как failed
                        if let Ok(conn) = db::get_connection(&db_pool) {
                            if let Err(db_err) = db::mark_task_failed(&conn, &task_id, &error_msg) {
                                log::error!(
                                    "Failed to mark task {} as failed in DB: {}",
                                    task_id,
                                    db_err
                                );
                            } else {
                                // Уведомляем администратора только если задача не превысила лимит попыток
                                if let Ok(conn) = db::get_connection(&db_pool) {
                                    if let Ok(Some(task_entry)) =
                                        db::get_task_by_id(&conn, &task_id)
                                    {
                                        if task_entry.retry_count < config::admin::MAX_TASK_RETRIES
                                        {
                                            notify_admin_task_failed(
                                                bot.clone(),
                                                Arc::clone(&db_pool),
                                                &task_id,
                                                task_chat_id.0,
                                                &task_url,
                                                &error_msg,
                                            )
                                            .await;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                log::info!("Task {} processing finished, permit released", task_id);
                // Permit is automatically released when _permit goes out of scope
            });
        }
    }
}

#[cfg(test)]
mod tests {
    pub use doradura::download::queue::DownloadQueue;
    pub use doradura::download::queue::DownloadTask;

    #[tokio::test]
    async fn test_adding_and_retrieving_task() {
        let queue = DownloadQueue::new();
        let task = DownloadTask::new(
            "http://example.com/video.mp4".to_string(),
            teloxide::types::ChatId(123456789),
            None,
            true,
            "mp4".to_string(),
            Some("1080p".to_string()),
            None,
        );

        // Test adding a task to the queue
        queue.add_task(task.clone(), None).await;
        assert_eq!(queue.queue.lock().await.len(), 1);

        // Test retrieving a task from the queue
        let retrieved_task = queue
            .get_task()
            .await
            .expect("Should retrieve task from non-empty queue");
        assert_eq!(retrieved_task.url, "http://example.com/video.mp4");
        assert_eq!(retrieved_task.chat_id, teloxide::types::ChatId(123456789));
        assert!(retrieved_task.is_video);
    }

    #[tokio::test]
    async fn test_queue_empty_after_retrieval() {
        let queue = DownloadQueue::new();
        let task = DownloadTask::new(
            "http://example.com/audio.mp3".to_string(),
            teloxide::types::ChatId(987654321),
            None,
            false,
            "mp3".to_string(),
            None,
            Some("320k".to_string()),
        );

        queue.add_task(task, None).await;
        assert_eq!(queue.queue.lock().await.len(), 1);

        // After retrieving, the queue should be empty
        let _retrieved_task = queue
            .get_task()
            .await
            .expect("Should retrieve task that was just added");
        assert!(queue.queue.lock().await.is_empty());
    }

    #[tokio::test]
    async fn test_multiple_tasks_handling() {
        let queue = DownloadQueue::new();
        let task1 = DownloadTask::new(
            "http://example.com/second.mp4".to_string(),
            teloxide::types::ChatId(111111111),
            None,
            true,
            "mp4".to_string(),
            Some("720p".to_string()),
            None,
        );
        let task2 = DownloadTask::new(
            "http://example.com/second.mp4".to_string(),
            teloxide::types::ChatId(111111111),
            None,
            false,
            "mp3".to_string(),
            None,
            Some("256k".to_string()),
        );
        queue.add_task(task2, None).await;
        queue.add_task(task1, None).await;

        // Check the count after adding tasks
        assert_eq!(queue.queue.lock().await.len(), 2);

        // Retrieve tasks and check the order and properties
        let first_retrieved_task = queue
            .get_task()
            .await
            .expect("Should retrieve first task from queue");
        assert_eq!(first_retrieved_task.url, "http://example.com/second.mp4");
        assert_eq!(
            first_retrieved_task.chat_id,
            teloxide::types::ChatId(111111111)
        );
        assert!(!first_retrieved_task.is_video);

        let second_retrieved_task = queue
            .get_task()
            .await
            .expect("Should retrieve second task from queue");
        assert_eq!(second_retrieved_task.url, "http://example.com/second.mp4");
        assert_eq!(
            second_retrieved_task.chat_id,
            teloxide::types::ChatId(111111111)
        );
        assert!(second_retrieved_task.is_video);

        // After retrieving all tasks, the queue should be empty
        assert!(queue.queue.lock().await.is_empty());
    }

    #[tokio::test]
    async fn test_queue_empty_initially() {
        let queue = DownloadQueue::new();
        assert!(queue.queue.lock().await.is_empty());
    }
}
