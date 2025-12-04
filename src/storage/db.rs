use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Connection, Result};

/// Структура, представляющая пользователя в базе данных.
pub struct User {
    /// Telegram ID пользователя
    pub telegram_id: i64,
    /// Имя пользователя (username) в Telegram, если доступно
    pub username: Option<String>,
    /// План пользователя (например, "free", "premium")
    pub plan: String,
    /// Предпочитаемый формат загрузки: "mp3", "mp4", "srt", "txt"
    pub download_format: String,
    /// Флаг загрузки субтитров (0 - отключено, 1 - включено)
    pub download_subtitles: i32,
    /// Качество видео: "best", "1080p", "720p", "480p", "360p"
    pub video_quality: String,
    /// Битрейт аудио: "128k", "192k", "256k", "320k"
    pub audio_bitrate: String,
    /// Тип отправки видео: 0 = Media (send_video), 1 = Document (send_document)
    pub send_as_document: i32,
    /// Тип отправки аудио: 0 = Media (send_audio), 1 = Document (send_document)
    pub send_audio_as_document: i32,
    /// Дата истечения подписки (None для Free или бессрочных подписок)
    pub subscription_expires_at: Option<String>,
    /// Telegram payment charge ID для управления подписками через Bot API
    pub telegram_charge_id: Option<String>,
}

impl User {
    /// Возвращает Telegram ID пользователя.
    ///
    /// # Returns
    ///
    /// Telegram ID пользователя.
    pub fn telegram_id(&self) -> i64 {
        self.telegram_id
    }

    /// Возвращает предпочитаемый формат загрузки пользователя.
    ///
    /// # Returns
    ///
    /// Формат загрузки: "mp3", "mp4", "srt", "txt"
    pub fn download_format(&self) -> &str {
        &self.download_format
    }
}

pub type DbPool = Pool<SqliteConnectionManager>;
pub type DbConnection = PooledConnection<SqliteConnectionManager>;

/// Create a new database connection pool
///
/// Initializes a connection pool with up to 10 connections and runs schema migrations.
///
/// # Arguments
///
/// * `database_path` - Path to SQLite database file
///
/// # Returns
///
/// Returns a `DbPool` on success or an `r2d2::Error` if pool creation fails.
///
/// # Example
///
/// ```no_run
/// use doradura::db;
///
/// let pool = db::create_pool("database.sqlite")?;
/// ```
pub fn create_pool(database_path: &str) -> Result<DbPool, r2d2::Error> {
    let manager = SqliteConnectionManager::file(database_path);
    let pool = Pool::builder()
        .max_size(10) // Maximum 10 connections in the pool
        .build(manager)?;

    // Ensure schema is up to date on first connection
    let conn = pool.get()?;
    if let Err(e) = migrate_schema(&conn) {
        log::warn!("Failed to migrate schema: {}", e);
        // Don't fail on migration errors, as they might be expected
    }

    Ok(pool)
}

/// Get a connection from the pool
///
/// Retrieves a connection from the connection pool. The connection is automatically
/// returned to the pool when dropped.
///
/// # Arguments
///
/// * `pool` - Database connection pool
///
/// # Returns
///
/// Returns a `DbConnection` on success or an `r2d2::Error` if no connection is available.
///
/// # Example
///
/// ```no_run
/// use doradura::db;
///
/// let pool = db::create_pool("database.sqlite")?;
/// let conn = db::get_connection(&pool)?;
/// // Use connection...
/// ```
pub fn get_connection(pool: &DbPool) -> Result<DbConnection, r2d2::Error> {
    pool.get()
}

/// Legacy function for backward compatibility (deprecated)
/// Use get_connection(&pool) instead
#[deprecated(note = "Use get_connection(&pool) instead")]
pub fn get_connection_legacy() -> Result<Connection> {
    let conn = Connection::open("database.sqlite")?;
    migrate_schema(&conn)?;
    Ok(conn)
}

/// Migrate database schema to ensure all required columns exist
/// This function safely adds missing columns to existing tables
fn migrate_schema(conn: &rusqlite::Connection) -> Result<()> {
    // First, check if users table exists
    let table_exists: bool = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='users'",
        [],
        |row| Ok(row.get::<_, i32>(0)? > 0),
    )?;

    if !table_exists {
        // Table doesn't exist yet, migration.sql will create it with all columns
        return Ok(());
    }

    // Table exists, check if columns exist
    let mut stmt = conn.prepare("PRAGMA table_info(users)")?;
    let rows = stmt.query_map([], |row| {
        Ok(row.get::<_, String>(1)?) // column name
    })?;

    let mut columns = Vec::new();
    for row in rows {
        columns.push(row?);
    }

    // Add download_format if it doesn't exist
    if !columns.contains(&"download_format".to_string()) {
        log::info!("Adding missing column: download_format to users table");
        if let Err(e) = conn.execute(
            "ALTER TABLE users ADD COLUMN download_format TEXT DEFAULT 'mp3'",
            [],
        ) {
            log::warn!("Failed to add download_format column: {}", e);
        }
    }

    // Add download_subtitles if it doesn't exist
    if !columns.contains(&"download_subtitles".to_string()) {
        log::info!("Adding missing column: download_subtitles to users table");
        if let Err(e) = conn.execute(
            "ALTER TABLE users ADD COLUMN download_subtitles INTEGER DEFAULT 0",
            [],
        ) {
            log::warn!("Failed to add download_subtitles column: {}", e);
        }
    }

    // Add video_quality if it doesn't exist
    if !columns.contains(&"video_quality".to_string()) {
        log::info!("Adding missing column: video_quality to users table");
        if let Err(e) = conn.execute(
            "ALTER TABLE users ADD COLUMN video_quality TEXT DEFAULT 'best'",
            [],
        ) {
            log::warn!("Failed to add video_quality column: {}", e);
        }
    }

    // Add audio_bitrate if it doesn't exist
    if !columns.contains(&"audio_bitrate".to_string()) {
        log::info!("Adding missing column: audio_bitrate to users table");
        if let Err(e) = conn.execute(
            "ALTER TABLE users ADD COLUMN audio_bitrate TEXT DEFAULT '320k'",
            [],
        ) {
            log::warn!("Failed to add audio_bitrate column: {}", e);
        }
    }

    // Add subscription_expires_at if it doesn't exist
    if !columns.contains(&"subscription_expires_at".to_string()) {
        log::info!("Adding missing column: subscription_expires_at to users table");
        if let Err(e) = conn.execute(
            "ALTER TABLE users ADD COLUMN subscription_expires_at DATETIME DEFAULT NULL",
            [],
        ) {
            log::warn!("Failed to add subscription_expires_at column: {}", e);
        }
    }

    // Add send_as_document if it doesn't exist
    if !columns.contains(&"send_as_document".to_string()) {
        log::info!("Adding missing column: send_as_document to users table");
        if let Err(e) = conn.execute(
            "ALTER TABLE users ADD COLUMN send_as_document INTEGER DEFAULT 0",
            [],
        ) {
            log::warn!("Failed to add send_as_document column: {}", e);
        }
    }

    // Add send_audio_as_document if it doesn't exist
    if !columns.contains(&"send_audio_as_document".to_string()) {
        log::info!("Adding missing column: send_audio_as_document to users table");
        if let Err(e) = conn.execute(
            "ALTER TABLE users ADD COLUMN send_audio_as_document INTEGER DEFAULT 0",
            [],
        ) {
            log::warn!("Failed to add send_audio_as_document column: {}", e);
        }
    }

    // Add telegram_charge_id if it doesn't exist
    if !columns.contains(&"telegram_charge_id".to_string()) {
        log::info!("Adding missing column: telegram_charge_id to users table");
        if let Err(e) = conn.execute(
            "ALTER TABLE users ADD COLUMN telegram_charge_id TEXT DEFAULT NULL",
            [],
        ) {
            log::warn!("Failed to add telegram_charge_id column: {}", e);
        }
    }

    // Create url_cache table if it doesn't exist
    let url_cache_exists: bool = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='url_cache'",
        [],
        |row| Ok(row.get::<_, i32>(0)? > 0),
    )?;

    if !url_cache_exists {
        log::info!("Creating url_cache table");
        if let Err(e) = conn.execute(
            "CREATE TABLE IF NOT EXISTS url_cache (
                id TEXT PRIMARY KEY,
                url TEXT NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                expires_at DATETIME NOT NULL
            )",
            [],
        ) {
            log::warn!("Failed to create url_cache table: {}", e);
        } else {
            // Create index for faster lookups
            if let Err(e) = conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_url_cache_expires_at ON url_cache(expires_at)",
                [],
            ) {
                log::warn!("Failed to create index on url_cache: {}", e);
            }
        }
    }

    Ok(())
}

/// Создает нового пользователя в базе данных.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
/// * `username` - Имя пользователя (опционально)
///
/// # Returns
///
/// Возвращает `Ok(())` при успехе или ошибку базы данных.
///
/// # Errors
///
/// Возвращает ошибку если пользователь с таким ID уже существует или произошла ошибка БД.
pub fn create_user(conn: &DbConnection, telegram_id: i64, username: Option<String>) -> Result<()> {
    conn.execute(
        "INSERT INTO users (telegram_id, username, download_format, download_subtitles, video_quality, audio_bitrate, send_as_document, send_audio_as_document, telegram_charge_id) VALUES (?1, ?2, 'mp3', 0, 'best', '320k', 0, 0, NULL)",
        &[&telegram_id as &dyn rusqlite::ToSql, &username as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Получает пользователя из базы данных по Telegram ID.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
///
/// # Returns
///
/// Возвращает `Ok(Some(User))` если пользователь найден, `Ok(None)` если не найден,
/// или ошибку базы данных.
pub fn get_user(conn: &DbConnection, telegram_id: i64) -> Result<Option<User>> {
    let mut stmt = conn.prepare("SELECT telegram_id, username, plan, download_format, download_subtitles, video_quality, audio_bitrate, send_as_document, send_audio_as_document, subscription_expires_at, telegram_charge_id FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query(&[&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        let telegram_id: i64 = row.get(0)?;
        let username: Option<String> = row.get(1)?;
        let plan: String = row.get(2)?;
        let download_format: String = row.get(3)?;
        let download_subtitles: i32 = row.get(4)?;
        let video_quality: String = row.get(5).unwrap_or_else(|_| "best".to_string());
        let audio_bitrate: String = row.get(6).unwrap_or_else(|_| "320k".to_string());
        let send_as_document: i32 = row.get(7).unwrap_or(0);
        let send_audio_as_document: i32 = row.get(8).unwrap_or(0);
        let subscription_expires_at: Option<String> = row.get(9).ok();
        let telegram_charge_id: Option<String> = row.get(10).ok();

        Ok(Some(User {
            telegram_id,
            username,
            plan,
            download_format,
            download_subtitles,
            video_quality,
            audio_bitrate,
            send_as_document,
            send_audio_as_document,
            subscription_expires_at,
            telegram_charge_id,
        }))
    } else {
        Ok(None)
    }
}

/// Обновляет план пользователя.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
/// * `plan` - Новый план пользователя (например, "free", "premium")
///
/// # Returns
///
/// Возвращает `Ok(())` при успехе или ошибку базы данных.
pub fn update_user_plan(conn: &DbConnection, telegram_id: i64, plan: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET plan = ?1 WHERE telegram_id = ?2",
        &[
            &plan as &dyn rusqlite::ToSql,
            &telegram_id as &dyn rusqlite::ToSql,
        ],
    )?;
    Ok(())
}

/// Обновляет план пользователя и устанавливает дату окончания подписки.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
/// * `plan` - Новый план пользователя (например, "free", "premium", "vip")
/// * `days` - Количество дней действия подписки (None для бессрочной/free)
///
/// # Returns
///
/// Возвращает `Ok(())` при успехе или ошибку базы данных.
pub fn update_user_plan_with_expiry(
    conn: &DbConnection,
    telegram_id: i64,
    plan: &str,
    days: Option<i32>,
) -> Result<()> {
    if let Some(days_count) = days {
        // Устанавливаем дату окончания на N дней вперед от текущей даты
        conn.execute(
            "UPDATE users SET plan = ?1, subscription_expires_at = datetime('now', '+' || ?2 || ' days') WHERE telegram_id = ?3",
            &[&plan as &dyn rusqlite::ToSql, &days_count as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
        )?;
    } else {
        // Для free плана или бессрочных подписок, убираем дату окончания
        conn.execute(
            "UPDATE users SET plan = ?1, subscription_expires_at = NULL WHERE telegram_id = ?2",
            &[
                &plan as &dyn rusqlite::ToSql,
                &telegram_id as &dyn rusqlite::ToSql,
            ],
        )?;
    }
    Ok(())
}

/// Проверяет и обновляет истекшие подписки, понижая их до free.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
///
/// # Returns
///
/// Возвращает количество обновленных пользователей.
pub fn expire_old_subscriptions(conn: &DbConnection) -> Result<usize> {
    let count = conn.execute(
        "UPDATE users SET plan = 'free', subscription_expires_at = NULL
         WHERE subscription_expires_at IS NOT NULL
         AND subscription_expires_at < datetime('now')
         AND plan != 'free'",
        [],
    )?;

    if count > 0 {
        log::info!("Expired {} subscription(s)", count);
    }

    Ok(count)
}

/// Логирует запрос пользователя в историю запросов.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `user_id` - Telegram ID пользователя
/// * `request_text` - Текст запроса пользователя (обычно URL)
///
/// # Returns
///
/// Возвращает `Ok(())` при успехе или ошибку базы данных.
pub fn log_request(conn: &DbConnection, user_id: i64, request_text: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO request_history (user_id, request_text) VALUES (?1, ?2)",
        &[
            &user_id as &dyn rusqlite::ToSql,
            &request_text as &dyn rusqlite::ToSql,
        ],
    )?;
    Ok(())
}

/// Получает предпочитаемый формат загрузки пользователя.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
///
/// # Returns
///
/// Возвращает формат загрузки ("mp3", "mp4", "srt", "txt") или "mp3" по умолчанию,
/// если пользователь не найден или произошла ошибка.
pub fn get_user_download_format(conn: &DbConnection, telegram_id: i64) -> Result<String> {
    let mut stmt = conn.prepare("SELECT download_format FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query(&[&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(row.get(0)?)
    } else {
        Ok("mp3".to_string())
    }
}

/// Устанавливает предпочитаемый формат загрузки пользователя.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
/// * `format` - Формат загрузки: "mp3", "mp4", "srt", "txt"
///
/// # Returns
///
/// Возвращает `Ok(())` при успехе или ошибку базы данных.
pub fn set_user_download_format(conn: &DbConnection, telegram_id: i64, format: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET download_format = ?1 WHERE telegram_id = ?2",
        &[
            &format as &dyn rusqlite::ToSql,
            &telegram_id as &dyn rusqlite::ToSql,
        ],
    )?;
    Ok(())
}

/// Получает настройку загрузки субтитров пользователя.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
///
/// # Returns
///
/// Возвращает `true` если загрузка субтитров включена, `false` если отключена или пользователь не найден.
pub fn get_user_download_subtitles(conn: &DbConnection, telegram_id: i64) -> Result<bool> {
    let mut stmt = conn.prepare("SELECT download_subtitles FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query(&[&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        let subtitles: i32 = row.get(0)?;
        Ok(subtitles == 1)
    } else {
        Ok(false)
    }
}

/// Устанавливает настройку загрузки субтитров пользователя.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
/// * `enabled` - Включить (`true`) или отключить (`false`) загрузку субтитров
///
/// # Returns
///
/// Возвращает `Ok(())` при успехе или ошибку базы данных.
pub fn set_user_download_subtitles(
    conn: &DbConnection,
    telegram_id: i64,
    enabled: bool,
) -> Result<()> {
    let value = if enabled { 1 } else { 0 };
    conn.execute(
        "UPDATE users SET download_subtitles = ?1 WHERE telegram_id = ?2",
        &[
            &value as &dyn rusqlite::ToSql,
            &telegram_id as &dyn rusqlite::ToSql,
        ],
    )?;
    Ok(())
}

/// Получает качество видео пользователя.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
///
/// # Returns
///
/// Возвращает качество видео ("best", "1080p", "720p", "480p", "360p") или "best" по умолчанию.
pub fn get_user_video_quality(conn: &DbConnection, telegram_id: i64) -> Result<String> {
    let mut stmt = conn.prepare("SELECT video_quality FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query(&[&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(row.get(0).unwrap_or_else(|_| "best".to_string()))
    } else {
        Ok("best".to_string())
    }
}

/// Устанавливает качество видео пользователя.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
/// * `quality` - Качество видео: "best", "1080p", "720p", "480p", "360p"
///
/// # Returns
///
/// Возвращает `Ok(())` при успехе или ошибку базы данных.
pub fn set_user_video_quality(conn: &DbConnection, telegram_id: i64, quality: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET video_quality = ?1 WHERE telegram_id = ?2",
        &[
            &quality as &dyn rusqlite::ToSql,
            &telegram_id as &dyn rusqlite::ToSql,
        ],
    )?;
    Ok(())
}

/// Получает тип отправки видео пользователя (0 = Media, 1 = Document).
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
///
/// # Returns
///
/// Возвращает `Ok(0)` для Media (send_video) или `Ok(1)` для Document (send_document).
/// По умолчанию возвращает 0 (Media).
pub fn get_user_send_as_document(conn: &DbConnection, telegram_id: i64) -> Result<i32> {
    let mut stmt = conn.prepare("SELECT send_as_document FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query(&[&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(row.get(0).unwrap_or(0))
    } else {
        Ok(0) // Default to Media
    }
}

/// Устанавливает тип отправки видео пользователя.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
/// * `send_as_document` - 0 = Media (send_video), 1 = Document (send_document)
///
/// # Returns
///
/// Возвращает `Ok(())` при успехе или ошибку базы данных.
pub fn set_user_send_as_document(
    conn: &DbConnection,
    telegram_id: i64,
    send_as_document: i32,
) -> Result<()> {
    conn.execute(
        "UPDATE users SET send_as_document = ?1 WHERE telegram_id = ?2",
        &[
            &send_as_document as &dyn rusqlite::ToSql,
            &telegram_id as &dyn rusqlite::ToSql,
        ],
    )?;
    Ok(())
}

/// Получает тип отправки аудио пользователя (0 = Media, 1 = Document).
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
///
/// # Returns
///
/// Возвращает `Ok(0)` для Media (send_audio) или `Ok(1)` для Document (send_document).
/// По умолчанию возвращает 0 (Media).
pub fn get_user_send_audio_as_document(conn: &DbConnection, telegram_id: i64) -> Result<i32> {
    let mut stmt =
        conn.prepare("SELECT send_audio_as_document FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query(&[&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(row.get(0).unwrap_or(0))
    } else {
        Ok(0) // Default to Media
    }
}

/// Устанавливает тип отправки аудио пользователя.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
/// * `send_audio_as_document` - 0 = Media (send_audio), 1 = Document (send_document)
///
/// # Returns
///
/// Возвращает `Ok(())` при успехе или ошибку базы данных.
pub fn set_user_send_audio_as_document(
    conn: &DbConnection,
    telegram_id: i64,
    send_audio_as_document: i32,
) -> Result<()> {
    conn.execute(
        "UPDATE users SET send_audio_as_document = ?1 WHERE telegram_id = ?2",
        &[
            &send_audio_as_document as &dyn rusqlite::ToSql,
            &telegram_id as &dyn rusqlite::ToSql,
        ],
    )?;
    Ok(())
}

/// Получает битрейт аудио пользователя.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
///
/// # Returns
///
/// Возвращает битрейт аудио ("128k", "192k", "256k", "320k") или "320k" по умолчанию.
pub fn get_user_audio_bitrate(conn: &DbConnection, telegram_id: i64) -> Result<String> {
    let mut stmt = conn.prepare("SELECT audio_bitrate FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query(&[&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(row.get(0).unwrap_or_else(|_| "320k".to_string()))
    } else {
        Ok("320k".to_string())
    }
}

/// Устанавливает битрейт аудио пользователя.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
/// * `bitrate` - Битрейт аудио: "128k", "192k", "256k", "320k"
///
/// # Returns
///
/// Возвращает `Ok(())` при успехе или ошибку базы данных.
pub fn set_user_audio_bitrate(conn: &DbConnection, telegram_id: i64, bitrate: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET audio_bitrate = ?1 WHERE telegram_id = ?2",
        &[
            &bitrate as &dyn rusqlite::ToSql,
            &telegram_id as &dyn rusqlite::ToSql,
        ],
    )?;
    Ok(())
}

/// Структура, представляющая запись истории загрузок.
#[derive(Debug, Clone)]
pub struct DownloadHistoryEntry {
    /// ID записи
    pub id: i64,
    /// URL загруженного контента
    pub url: String,
    /// Название трека/видео
    pub title: String,
    /// Формат загрузки (mp3, mp4, srt, txt)
    pub format: String,
    /// Дата и время загрузки
    pub downloaded_at: String,
}

/// Сохраняет запись в историю загрузок.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
/// * `url` - URL загруженного контента
/// * `title` - Название трека/видео
/// * `format` - Формат загрузки (mp3, mp4, srt, txt)
///
/// # Returns
///
/// Возвращает `Ok(())` при успехе или ошибку базы данных.
pub fn save_download_history(
    conn: &DbConnection,
    telegram_id: i64,
    url: &str,
    title: &str,
    format: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO download_history (user_id, url, title, format) VALUES (?1, ?2, ?3, ?4)",
        &[
            &telegram_id as &dyn rusqlite::ToSql,
            &url as &dyn rusqlite::ToSql,
            &title as &dyn rusqlite::ToSql,
            &format as &dyn rusqlite::ToSql,
        ],
    )?;
    Ok(())
}

/// Получает последние N записей истории загрузок пользователя.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
/// * `limit` - Максимальное количество записей (по умолчанию 20)
///
/// # Returns
///
/// Возвращает `Ok(Vec<DownloadHistoryEntry>)` с записями истории или ошибку базы данных.
pub fn get_download_history(
    conn: &DbConnection,
    telegram_id: i64,
    limit: Option<i32>,
) -> Result<Vec<DownloadHistoryEntry>> {
    let limit = limit.unwrap_or(20);
    let mut stmt = conn.prepare(
        "SELECT id, url, title, format, downloaded_at FROM download_history
         WHERE user_id = ? ORDER BY downloaded_at DESC LIMIT ?",
    )?;
    let rows = stmt.query_map(
        &[
            &telegram_id as &dyn rusqlite::ToSql,
            &limit as &dyn rusqlite::ToSql,
        ],
        |row| {
            Ok(DownloadHistoryEntry {
                id: row.get(0)?,
                url: row.get(1)?,
                title: row.get(2)?,
                format: row.get(3)?,
                downloaded_at: row.get(4)?,
            })
        },
    )?;

    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }
    Ok(entries)
}

/// Удаляет запись из истории загрузок.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
/// * `entry_id` - ID записи для удаления
///
/// # Returns
///
/// Возвращает `Ok(true)` если запись была удалена, `Ok(false)` если запись не найдена,
/// или ошибку базы данных.
pub fn delete_download_history_entry(
    conn: &DbConnection,
    telegram_id: i64,
    entry_id: i64,
) -> Result<bool> {
    let rows_affected = conn.execute(
        "DELETE FROM download_history WHERE id = ?1 AND user_id = ?2",
        &[
            &entry_id as &dyn rusqlite::ToSql,
            &telegram_id as &dyn rusqlite::ToSql,
        ],
    )?;
    Ok(rows_affected > 0)
}

/// Получает запись истории загрузок по ID.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
/// * `entry_id` - ID записи
///
/// # Returns
///
/// Возвращает `Ok(Some(DownloadHistoryEntry))` если запись найдена, `Ok(None)` если не найдена,
/// или ошибку базы данных.
pub fn get_download_history_entry(
    conn: &DbConnection,
    telegram_id: i64,
    entry_id: i64,
) -> Result<Option<DownloadHistoryEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, url, title, format, downloaded_at FROM download_history
         WHERE id = ?1 AND user_id = ?2",
    )?;
    let mut rows = stmt.query_map(
        &[
            &entry_id as &dyn rusqlite::ToSql,
            &telegram_id as &dyn rusqlite::ToSql,
        ],
        |row| {
            Ok(DownloadHistoryEntry {
                id: row.get(0)?,
                url: row.get(1)?,
                title: row.get(2)?,
                format: row.get(3)?,
                downloaded_at: row.get(4)?,
            })
        },
    )?;

    if let Some(row) = rows.next() {
        Ok(Some(row?))
    } else {
        Ok(None)
    }
}

/// Структура статистики пользователя
#[derive(Debug, Clone)]
pub struct UserStats {
    pub total_downloads: i64,
    pub total_size: i64, // в байтах (приблизительно)
    pub active_days: i64,
    pub top_artists: Vec<(String, i64)>,     // (artist, count)
    pub top_formats: Vec<(String, i64)>,     // (format, count)
    pub activity_by_day: Vec<(String, i64)>, // (date, count) для последних 7 дней
}

/// Получает статистику пользователя
pub fn get_user_stats(conn: &DbConnection, telegram_id: i64) -> Result<UserStats> {
    // Общее количество загрузок
    let total_downloads: i64 = conn.query_row(
        "SELECT COUNT(*) FROM download_history WHERE user_id = ?",
        &[&telegram_id as &dyn rusqlite::ToSql],
        |row| row.get(0),
    )?;

    // Приблизительный общий размер (очень грубая оценка: mp3 ~5MB, mp4 ~50MB)
    let total_size: i64 = match conn.query_row(
        "SELECT
            SUM(CASE
                WHEN format = 'mp3' THEN 5000000
                WHEN format = 'mp4' THEN 50000000
                ELSE 1000000
            END)
        FROM download_history WHERE user_id = ?",
        &[&telegram_id as &dyn rusqlite::ToSql],
        |row| row.get::<_, Option<i64>>(0),
    ) {
        Ok(Some(size)) => size,
        Ok(None) => 0,
        Err(e) => return Err(e),
    };

    // Количество дней активности
    let active_days: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT DATE(downloaded_at)) FROM download_history WHERE user_id = ?",
        &[&telegram_id as &dyn rusqlite::ToSql],
        |row| row.get(0),
    )?;

    // Топ-5 исполнителей (парсим из title: "Artist - Song")
    let mut stmt = conn.prepare(
        "SELECT title FROM download_history WHERE user_id = ? ORDER BY downloaded_at DESC LIMIT 100"
    )?;
    let rows = stmt.query_map(&[&telegram_id as &dyn rusqlite::ToSql], |row| {
        Ok(row.get::<_, String>(0)?)
    })?;

    let mut artist_counts: std::collections::HashMap<String, i64> =
        std::collections::HashMap::new();
    for row in rows {
        if let Ok(title) = row {
            // Пытаемся извлечь исполнителя из формата "Artist - Song"
            if let Some(pos) = title.find(" - ") {
                let artist = title[..pos].trim().to_string();
                if !artist.is_empty() {
                    *artist_counts.entry(artist).or_insert(0) += 1;
                }
            }
        }
    }

    let mut top_artists: Vec<(String, i64)> = artist_counts.into_iter().collect();
    top_artists.sort_by(|a, b| b.1.cmp(&a.1));
    top_artists.truncate(5);

    // Топ форматов
    let mut stmt = conn.prepare(
        "SELECT format, COUNT(*) as cnt FROM download_history
         WHERE user_id = ? GROUP BY format ORDER BY cnt DESC LIMIT 5",
    )?;
    let rows = stmt.query_map(&[&telegram_id as &dyn rusqlite::ToSql], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;

    let mut top_formats = Vec::new();
    for row in rows {
        if let Ok((format, count)) = row {
            top_formats.push((format, count));
        }
    }

    // Активность по дням (последние 7 дней)
    let mut stmt = conn.prepare(
        "SELECT DATE(downloaded_at) as day, COUNT(*) as cnt
         FROM download_history
         WHERE user_id = ? AND downloaded_at >= datetime('now', '-7 days')
         GROUP BY DATE(downloaded_at)
         ORDER BY day DESC",
    )?;
    let rows = stmt.query_map(&[&telegram_id as &dyn rusqlite::ToSql], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;

    let mut activity_by_day = Vec::new();
    for row in rows {
        if let Ok((day, count)) = row {
            activity_by_day.push((day, count));
        }
    }

    Ok(UserStats {
        total_downloads,
        total_size,
        active_days,
        top_artists,
        top_formats,
        activity_by_day,
    })
}

/// Структура глобальной статистики
#[derive(Debug, Clone)]
pub struct GlobalStats {
    pub total_users: i64,
    pub total_downloads: i64,
    pub top_tracks: Vec<(String, i64)>,  // (title, count)
    pub top_formats: Vec<(String, i64)>, // (format, count)
}

/// Получает глобальную статистику бота
pub fn get_global_stats(conn: &DbConnection) -> Result<GlobalStats> {
    // Общее количество пользователей
    let total_users: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT user_id) FROM download_history",
        [],
        |row| row.get(0),
    )?;

    // Общее количество загрузок
    let total_downloads: i64 =
        conn.query_row("SELECT COUNT(*) FROM download_history", [], |row| {
            row.get(0)
        })?;

    // Топ-10 треков (по title)
    let mut stmt = conn.prepare(
        "SELECT title, COUNT(*) as cnt FROM download_history
         GROUP BY title ORDER BY cnt DESC LIMIT 10",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;

    let mut top_tracks = Vec::new();
    for row in rows {
        if let Ok((title, count)) = row {
            top_tracks.push((title, count));
        }
    }

    // Топ форматов
    let mut stmt = conn.prepare(
        "SELECT format, COUNT(*) as cnt FROM download_history
         GROUP BY format ORDER BY cnt DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;

    let mut top_formats = Vec::new();
    for row in rows {
        if let Ok((format, count)) = row {
            top_formats.push((format, count));
        }
    }

    Ok(GlobalStats {
        total_users,
        total_downloads,
        top_tracks,
        top_formats,
    })
}

/// Получает всю историю загрузок пользователя для экспорта
pub fn get_all_download_history(
    conn: &DbConnection,
    telegram_id: i64,
) -> Result<Vec<DownloadHistoryEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, url, title, format, downloaded_at FROM download_history
         WHERE user_id = ? ORDER BY downloaded_at DESC",
    )?;
    let rows = stmt.query_map(&[&telegram_id as &dyn rusqlite::ToSql], |row| {
        Ok(DownloadHistoryEntry {
            id: row.get(0)?,
            url: row.get(1)?,
            title: row.get(2)?,
            format: row.get(3)?,
            downloaded_at: row.get(4)?,
        })
    })?;

    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }
    Ok(entries)
}

/// Получает список всех пользователей из базы данных.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
///
/// # Returns
///
/// Возвращает `Ok(Vec<User>)` со всеми пользователями или ошибку базы данных.
pub fn get_all_users(conn: &DbConnection) -> Result<Vec<User>> {
    let mut stmt = conn.prepare("SELECT telegram_id, username, plan, download_format, download_subtitles, video_quality, audio_bitrate, send_as_document, send_audio_as_document, subscription_expires_at, telegram_charge_id FROM users ORDER BY telegram_id")?;
    let rows = stmt.query_map([], |row| {
        Ok(User {
            telegram_id: row.get(0)?,
            username: row.get(1)?,
            plan: row.get(2)?,
            download_format: row.get(3)?,
            download_subtitles: row.get(4)?,
            video_quality: row.get(5).unwrap_or_else(|_| "best".to_string()),
            audio_bitrate: row.get(6).unwrap_or_else(|_| "320k".to_string()),
            send_as_document: row.get(7).unwrap_or(0),
            send_audio_as_document: row.get(8).unwrap_or(0),
            subscription_expires_at: row.get(9).ok(),
            telegram_charge_id: row.get(10).ok(),
        })
    })?;

    let mut users = Vec::new();
    for row in rows {
        users.push(row?);
    }
    Ok(users)
}

/// Структура задачи в очереди БД
#[derive(Debug, Clone)]
pub struct TaskQueueEntry {
    pub id: String,
    pub user_id: i64,
    pub url: String,
    pub format: String,
    pub is_video: bool,
    pub video_quality: Option<String>,
    pub audio_bitrate: Option<String>,
    pub priority: i32,
    pub status: String,
    pub error_message: Option<String>,
    pub retry_count: i32,
    pub created_at: String,
    pub updated_at: String,
}

/// Сохраняет задачу в очередь БД
pub fn save_task_to_queue(
    conn: &DbConnection,
    task_id: &str,
    user_id: i64,
    url: &str,
    format: &str,
    is_video: bool,
    video_quality: Option<&str>,
    audio_bitrate: Option<&str>,
    priority: i32,
) -> Result<()> {
    conn.execute(
        "INSERT INTO task_queue (id, user_id, url, format, is_video, video_quality, audio_bitrate, priority, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending')
         ON CONFLICT(id) DO UPDATE SET
         status = 'pending',
         updated_at = CURRENT_TIMESTAMP,
         retry_count = 0,
         error_message = NULL",
        &[
            &task_id as &dyn rusqlite::ToSql,
            &user_id as &dyn rusqlite::ToSql,
            &url as &dyn rusqlite::ToSql,
            &format as &dyn rusqlite::ToSql,
            &(if is_video { 1 } else { 0 }) as &dyn rusqlite::ToSql,
            &video_quality as &dyn rusqlite::ToSql,
            &audio_bitrate as &dyn rusqlite::ToSql,
            &priority as &dyn rusqlite::ToSql,
        ],
    )?;
    Ok(())
}

/// Обновляет статус задачи
pub fn update_task_status(
    conn: &DbConnection,
    task_id: &str,
    status: &str,
    error_message: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE task_queue SET status = ?1, error_message = ?2, updated_at = CURRENT_TIMESTAMP WHERE id = ?3",
        &[
            &status as &dyn rusqlite::ToSql,
            &error_message as &dyn rusqlite::ToSql,
            &task_id as &dyn rusqlite::ToSql,
        ],
    )?;
    Ok(())
}

/// Увеличивает счетчик попыток и обновляет статус на failed
pub fn mark_task_failed(conn: &DbConnection, task_id: &str, error_message: &str) -> Result<()> {
    conn.execute(
        "UPDATE task_queue
         SET status = 'failed',
             error_message = ?1,
             retry_count = retry_count + 1,
             updated_at = CURRENT_TIMESTAMP
         WHERE id = ?2",
        &[
            &error_message as &dyn rusqlite::ToSql,
            &task_id as &dyn rusqlite::ToSql,
        ],
    )?;
    Ok(())
}

/// Получает все failed задачи для повторной обработки
pub fn get_failed_tasks(conn: &DbConnection, max_retries: i32) -> Result<Vec<TaskQueueEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, url, format, is_video, video_quality, audio_bitrate, priority, status, error_message, retry_count, created_at, updated_at
         FROM task_queue
         WHERE status = 'failed' AND retry_count < ?1
         ORDER BY priority DESC, created_at ASC"
    )?;
    let rows = stmt.query_map(&[&max_retries as &dyn rusqlite::ToSql], |row| {
        Ok(TaskQueueEntry {
            id: row.get(0)?,
            user_id: row.get(1)?,
            url: row.get(2)?,
            format: row.get(3)?,
            is_video: row.get::<_, i32>(4)? == 1,
            video_quality: row.get(5)?,
            audio_bitrate: row.get(6)?,
            priority: row.get(7)?,
            status: row.get(8)?,
            error_message: row.get(9)?,
            retry_count: row.get(10)?,
            created_at: row.get(11)?,
            updated_at: row.get(12)?,
        })
    })?;

    let mut tasks = Vec::new();
    for row in rows {
        tasks.push(row?);
    }
    Ok(tasks)
}

/// Получает задачу по ID
pub fn get_task_by_id(conn: &DbConnection, task_id: &str) -> Result<Option<TaskQueueEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, url, format, is_video, video_quality, audio_bitrate, priority, status, error_message, retry_count, created_at, updated_at
         FROM task_queue WHERE id = ?1"
    )?;
    let mut rows = stmt.query_map(&[&task_id as &dyn rusqlite::ToSql], |row| {
        Ok(TaskQueueEntry {
            id: row.get(0)?,
            user_id: row.get(1)?,
            url: row.get(2)?,
            format: row.get(3)?,
            is_video: row.get::<_, i32>(4)? == 1,
            video_quality: row.get(5)?,
            audio_bitrate: row.get(6)?,
            priority: row.get(7)?,
            status: row.get(8)?,
            error_message: row.get(9)?,
            retry_count: row.get(10)?,
            created_at: row.get(11)?,
            updated_at: row.get(12)?,
        })
    })?;

    if let Some(row) = rows.next() {
        Ok(Some(row?))
    } else {
        Ok(None)
    }
}

/// Помечает задачу как completed
pub fn mark_task_completed(conn: &DbConnection, task_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE task_queue SET status = 'completed', updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
        &[&task_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Помечает задачу как processing
pub fn mark_task_processing(conn: &DbConnection, task_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE task_queue SET status = 'processing', updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
        &[&task_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Обновляет telegram_charge_id пользователя (используется для управления подписками)
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
/// * `charge_id` - Telegram payment charge ID из успешного платежа
///
/// # Returns
///
/// Возвращает `Ok(())` при успехе или ошибку базы данных.
pub fn update_telegram_charge_id(
    conn: &DbConnection,
    telegram_id: i64,
    charge_id: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE users SET telegram_charge_id = ?1 WHERE telegram_id = ?2",
        &[
            &charge_id as &dyn rusqlite::ToSql,
            &telegram_id as &dyn rusqlite::ToSql,
        ],
    )?;
    Ok(())
}
