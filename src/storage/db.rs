use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Connection, Result};

use crate::storage::migrations;

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
    /// Дата истечения подписки (из таблицы subscriptions)
    pub subscription_expires_at: Option<String>,
    /// Telegram payment charge ID (из таблицы subscriptions)
    pub telegram_charge_id: Option<String>,
    /// Предпочитаемый язык пользователя (IETF tag, например, "ru", "en-US")
    pub language: String,
    /// Флаг рекуррентной подписки (автопродление) из таблицы subscriptions
    pub is_recurring: bool,
    /// Флаг вшивания субтитров в видео (0 - отключено, 1 - включено)
    pub burn_subtitles: i32,
}

/// Структура с данными подписки пользователя.
#[derive(Debug, Clone)]
pub struct Subscription {
    pub user_id: i64,
    pub plan: String,
    pub expires_at: Option<String>,
    pub telegram_charge_id: Option<String>,
    pub is_recurring: bool,
}

/// Структура с данными платежа (charge) из Telegram Stars.
/// Хранит полную информацию о платеже для бухгалтерии.
#[derive(Debug, Clone)]
pub struct Charge {
    pub id: i64,
    pub user_id: i64,
    pub plan: String,
    pub telegram_charge_id: String,
    pub provider_charge_id: Option<String>,
    pub currency: String,
    pub total_amount: i64,
    pub invoice_payload: String,
    pub is_recurring: bool,
    pub is_first_recurring: bool,
    pub subscription_expiration_date: Option<String>,
    pub payment_date: String,
    pub created_at: String,
}

/// Структура с данными отзыва пользователя.
#[derive(Debug, Clone)]
pub struct FeedbackMessage {
    pub id: i64,
    pub user_id: i64,
    pub username: Option<String>,
    pub first_name: String,
    pub message: String,
    pub status: String,
    pub admin_reply: Option<String>,
    pub created_at: String,
    pub replied_at: Option<String>,
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
/// use doradura::storage::db;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let pool = db::create_pool("database.sqlite")?;
/// # Ok(())
/// # }
/// ```
pub fn create_pool(database_path: &str) -> Result<DbPool, r2d2::Error> {
    let path = std::path::Path::new(database_path);
    let resolved_path = if path.is_absolute() {
        path.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(path)
    } else {
        path.to_path_buf()
    };
    log::info!("Using SQLite database at {}", resolved_path.display());

    // Run migrations before pool creation to avoid holding a pooled connection open
    match Connection::open(database_path) {
        Ok(mut conn) => {
            if let Err(e) = migrations::run_migrations(&mut conn) {
                log::warn!("Failed to run database migrations: {}", e);
            }
        }
        Err(e) => {
            log::warn!("Failed to open database for migrations: {}", e);
        }
    }

    let manager = SqliteConnectionManager::file(database_path);
    let pool = Pool::builder()
        .max_size(10) // Maximum 10 connections in the pool
        .build(manager)?;

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
/// use doradura::storage::db;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let pool = db::create_pool("database.sqlite")?;
/// let conn = db::get_connection(&pool)?;
/// // Use connection...
/// # Ok(())
/// # }
/// ```
pub fn get_connection(pool: &DbPool) -> Result<DbConnection, r2d2::Error> {
    pool.get()
}

/// Legacy function for backward compatibility (deprecated)
/// Use get_connection(&pool) instead
#[deprecated(note = "Use get_connection(&pool) instead")]
pub fn get_connection_legacy() -> Result<Connection> {
    let mut conn = Connection::open("database.sqlite")?;
    if let Err(e) = migrations::run_migrations(&mut conn) {
        log::warn!("Failed to run migrations: {}", e);
    }
    Ok(conn)
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
        "INSERT INTO users (telegram_id, username, download_format, download_subtitles, video_quality, audio_bitrate, language, send_as_document, send_audio_as_document) VALUES (?1, ?2, 'mp3', 0, 'best', '320k', 'en', 0, 0)",
        [
            &telegram_id as &dyn rusqlite::ToSql,
            &username as &dyn rusqlite::ToSql,
        ],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO subscriptions (user_id, plan) VALUES (?1, 'free')",
        [&telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Создает нового пользователя в базе данных с указанным языком.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
/// * `username` - Имя пользователя (опционально)
/// * `language` - Код языка (например, "ru", "en", "fr", "de")
///
/// # Returns
///
/// Возвращает `Ok(())` при успехе или ошибку базы данных.
///
/// # Errors
///
/// Возвращает ошибку если пользователь с таким ID уже существует или произошла ошибка БД.
pub fn create_user_with_language(
    conn: &DbConnection,
    telegram_id: i64,
    username: Option<String>,
    language: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO users (telegram_id, username, download_format, download_subtitles, video_quality, audio_bitrate, language, send_as_document, send_audio_as_document) VALUES (?1, ?2, 'mp3', 0, 'best', '320k', ?3, 0, 0)",
        [
            &telegram_id as &dyn rusqlite::ToSql,
            &username as &dyn rusqlite::ToSql,
            &language as &dyn rusqlite::ToSql,
        ],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO subscriptions (user_id, plan) VALUES (?1, 'free')",
        [&telegram_id as &dyn rusqlite::ToSql],
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
    let mut stmt = conn.prepare(
        "SELECT
            u.telegram_id,
            u.username,
            COALESCE(s.plan, u.plan) as plan,
            u.download_format,
            u.download_subtitles,
            u.video_quality,
            u.audio_bitrate,
            u.language,
            u.send_as_document,
            u.send_audio_as_document,
            s.expires_at as subscription_expires_at,
            s.telegram_charge_id as telegram_charge_id,
            COALESCE(s.is_recurring, 0) as is_recurring,
            COALESCE(u.burn_subtitles, 0) as burn_subtitles
        FROM users u
        LEFT JOIN subscriptions s ON s.user_id = u.telegram_id
        WHERE u.telegram_id = ?",
    )?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        let telegram_id: i64 = row.get(0)?;
        let username: Option<String> = row.get(1)?;
        let plan: String = row.get(2)?;
        let download_format: String = row.get(3)?;
        let download_subtitles: i32 = row.get(4)?;
        let video_quality: String = row.get(5).unwrap_or_else(|_| "best".to_string());
        let audio_bitrate: String = row.get(6).unwrap_or_else(|_| "320k".to_string());
        let language: String = row.get(7).unwrap_or_else(|_| "ru".to_string());
        let send_as_document: i32 = row.get(8).unwrap_or(0);
        let send_audio_as_document: i32 = row.get(9).unwrap_or(0);
        let subscription_expires_at: Option<String> = row.get(10)?;
        let telegram_charge_id: Option<String> = row.get(11)?;
        let is_recurring: bool = row.get::<_, i32>(12).unwrap_or(0) != 0;
        let burn_subtitles: i32 = row.get(13).unwrap_or(0);

        Ok(Some(User {
            telegram_id,
            username,
            plan,
            download_format,
            download_subtitles,
            video_quality,
            audio_bitrate,
            language,
            send_as_document,
            send_audio_as_document,
            subscription_expires_at,
            telegram_charge_id,
            is_recurring,
            burn_subtitles,
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
        "INSERT INTO subscriptions (user_id, plan)
         VALUES (?1, ?2)
         ON CONFLICT(user_id) DO UPDATE SET
            plan = excluded.plan,
            updated_at = CURRENT_TIMESTAMP",
        [&telegram_id as &dyn rusqlite::ToSql, &plan as &dyn rusqlite::ToSql],
    )?;
    conn.execute(
        "UPDATE users SET plan = ?1 WHERE telegram_id = ?2",
        [&plan as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
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
            "INSERT INTO subscriptions (user_id, plan, expires_at)
             VALUES (?1, ?2, datetime('now', '+' || ?3 || ' days'))
             ON CONFLICT(user_id) DO UPDATE SET
                plan = excluded.plan,
                expires_at = excluded.expires_at,
                updated_at = CURRENT_TIMESTAMP",
            [
                &telegram_id as &dyn rusqlite::ToSql,
                &plan as &dyn rusqlite::ToSql,
                &days_count as &dyn rusqlite::ToSql,
            ],
        )?;
        conn.execute(
            "UPDATE users SET plan = ?1 WHERE telegram_id = ?2",
            [&plan as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
        )?;
    } else {
        // Для free плана или бессрочных подписок, убираем дату окончания
        conn.execute(
            "INSERT INTO subscriptions (user_id, plan, expires_at)
             VALUES (?1, ?2, NULL)
             ON CONFLICT(user_id) DO UPDATE SET
                plan = excluded.plan,
                expires_at = NULL,
                updated_at = CURRENT_TIMESTAMP",
            [&telegram_id as &dyn rusqlite::ToSql, &plan as &dyn rusqlite::ToSql],
        )?;
        conn.execute(
            "UPDATE users SET plan = ?1 WHERE telegram_id = ?2",
            [&plan as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
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
    let expired_user_ids = {
        let mut stmt = conn.prepare(
            "SELECT user_id FROM subscriptions
             WHERE expires_at IS NOT NULL
             AND expires_at < datetime('now')
             AND plan != 'free'",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;

        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        ids
    };

    if expired_user_ids.is_empty() {
        return Ok(0);
    }

    conn.execute(
        "UPDATE subscriptions
         SET plan = 'free',
             expires_at = NULL,
             telegram_charge_id = NULL,
             is_recurring = 0,
             updated_at = CURRENT_TIMESTAMP
         WHERE user_id IN (
             SELECT user_id FROM subscriptions
             WHERE expires_at IS NOT NULL
               AND expires_at < datetime('now')
               AND plan != 'free'
         )",
        [],
    )?;

    for user_id in &expired_user_ids {
        conn.execute("UPDATE users SET plan = 'free' WHERE telegram_id = ?1", [user_id])?;
    }

    if !expired_user_ids.is_empty() {
        log::info!("Expired {} subscription(s)", expired_user_ids.len());
    }

    Ok(expired_user_ids.len())
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
        [&user_id as &dyn rusqlite::ToSql, &request_text as &dyn rusqlite::ToSql],
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
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

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
        [&format as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
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
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

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
pub fn set_user_download_subtitles(conn: &DbConnection, telegram_id: i64, enabled: bool) -> Result<()> {
    let value = if enabled { 1 } else { 0 };
    conn.execute(
        "UPDATE users SET download_subtitles = ?1 WHERE telegram_id = ?2",
        [&value as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Получает настройку вшивания субтитров в видео.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
///
/// # Returns
///
/// Возвращает `Ok(true)` если вшивание включено, `Ok(false)` если отключено,
/// или ошибку базы данных.
pub fn get_user_burn_subtitles(conn: &DbConnection, telegram_id: i64) -> Result<bool> {
    let mut stmt = conn.prepare("SELECT COALESCE(burn_subtitles, 0) FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        let burn_subtitles: i32 = row.get(0)?;
        Ok(burn_subtitles == 1)
    } else {
        Ok(false)
    }
}

/// Устанавливает настройку вшивания субтитров в видео.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
/// * `enabled` - Включить (`true`) или отключить (`false`) вшивание субтитров
///
/// # Returns
///
/// Возвращает `Ok(())` при успехе или ошибку базы данных.
pub fn set_user_burn_subtitles(conn: &DbConnection, telegram_id: i64, enabled: bool) -> Result<()> {
    let value = if enabled { 1 } else { 0 };
    conn.execute(
        "UPDATE users SET burn_subtitles = ?1 WHERE telegram_id = ?2",
        [&value as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
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
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

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
        [&quality as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
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
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

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
pub fn set_user_send_as_document(conn: &DbConnection, telegram_id: i64, send_as_document: i32) -> Result<()> {
    conn.execute(
        "UPDATE users SET send_as_document = ?1 WHERE telegram_id = ?2",
        [
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
    let mut stmt = conn.prepare("SELECT send_audio_as_document FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

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
        [
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
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

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
        [&bitrate as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Получает предпочтительный язык пользователя (IETF код, например, "en", "ru").
pub fn get_user_language(conn: &DbConnection, telegram_id: i64) -> Result<String> {
    let mut stmt = conn.prepare("SELECT language FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(row.get(0).unwrap_or_else(|_| "ru".to_string()))
    } else {
        Ok("ru".to_string())
    }
}

/// Устанавливает предпочтительный язык пользователя.
pub fn set_user_language(conn: &DbConnection, telegram_id: i64, language: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET language = ?1 WHERE telegram_id = ?2",
        [&language as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
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
    /// Telegram file_id (опционально)
    pub file_id: Option<String>,
    /// Автор трека/видео (опционально)
    pub author: Option<String>,
    /// Размер файла в байтах (опционально)
    pub file_size: Option<i64>,
    /// Длительность в секундах (опционально)
    pub duration: Option<i64>,
    /// Качество видео (опционально, для mp4)
    pub video_quality: Option<String>,
    /// Битрейт аудио (опционально, для mp3)
    pub audio_bitrate: Option<String>,
    /// Bot API base URL used when saving this entry (optional, for debugging)
    pub bot_api_url: Option<String>,
    /// Whether a local Bot API server was used (0/1, optional for older rows)
    pub bot_api_is_local: Option<i64>,
    /// ID исходного файла (для разбитых видео)
    pub source_id: Option<i64>,
    /// Номер части (для разбитых видео)
    pub part_index: Option<i32>,
}

fn current_bot_api_info() -> (Option<String>, i64) {
    let url = std::env::var("BOT_API_URL").ok();
    let is_local = url.as_deref().map(|u| !u.contains("api.telegram.org")).unwrap_or(false);
    (url, if is_local { 1 } else { 0 })
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
/// * `file_id` - Telegram file_id, если контент был отправлен в Telegram (опционально)
/// * `author` - Автор трека/видео (опционально)
/// * `file_size` - Размер файла в байтах (опционально)
/// * `duration` - Длительность в секундах (опционально)
/// * `video_quality` - Качество видео (опционально)
/// * `audio_bitrate` - Битрейт аудио (опционально)
/// * `source_id` - ID исходного файла (для разбитых видео)
/// * `part_index` - Номер части (для разбитых видео)
///
/// # Returns
///
/// Возвращает `Ok(id)` при успехе (ID вставленной записи) или ошибку базы данных.
pub fn save_download_history(
    conn: &DbConnection,
    telegram_id: i64,
    url: &str,
    title: &str,
    format: &str,
    file_id: Option<&str>,
    author: Option<&str>,
    file_size: Option<i64>,
    duration: Option<i64>,
    video_quality: Option<&str>,
    audio_bitrate: Option<&str>,
    source_id: Option<i64>,
    part_index: Option<i32>,
) -> Result<i64> {
    let (bot_api_url, bot_api_is_local) = current_bot_api_info();
    conn.execute(
        "INSERT INTO download_history (
            user_id, url, title, format, file_id, author, file_size, duration, video_quality, audio_bitrate,
            bot_api_url, bot_api_is_local, source_id, part_index
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        rusqlite::params![
            telegram_id,
            url,
            title,
            format,
            file_id,
            author,
            file_size,
            duration,
            video_quality,
            audio_bitrate,
            bot_api_url,
            bot_api_is_local,
            source_id,
            part_index
        ],
    )?;
    Ok(conn.last_insert_rowid())
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
        "SELECT id, url, title, format, downloaded_at, file_id, author, file_size, duration, video_quality, audio_bitrate,
                bot_api_url, bot_api_is_local, source_id, part_index
         FROM download_history
         WHERE user_id = ? ORDER BY downloaded_at DESC LIMIT ?",
    )?;
    let rows = stmt.query_map(rusqlite::params![telegram_id, limit], |row| {
        Ok(DownloadHistoryEntry {
            id: row.get(0)?,
            url: row.get(1)?,
            title: row.get(2)?,
            format: row.get(3)?,
            downloaded_at: row.get(4)?,
            file_id: row.get(5)?,
            author: row.get(6)?,
            file_size: row.get(7)?,
            duration: row.get(8)?,
            video_quality: row.get(9)?,
            audio_bitrate: row.get(10)?,
            bot_api_url: row.get(11)?,
            bot_api_is_local: row.get(12)?,
            source_id: row.get(13)?,
            part_index: row.get(14)?,
        })
    })?;

    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }
    Ok(entries)
}

/// Структура, представляющая файл с file_id для администратора.
#[derive(Debug, Clone)]
pub struct SentFile {
    /// ID записи
    pub id: i64,
    /// Telegram ID пользователя
    pub user_id: i64,
    /// Username пользователя (если доступен)
    pub username: Option<String>,
    /// URL загруженного контента
    pub url: String,
    /// Название файла
    pub title: String,
    /// Формат файла (mp3, mp4, srt, txt)
    pub format: String,
    /// Дата и время загрузки
    pub downloaded_at: String,
    /// Telegram file_id
    pub file_id: String,
}

/// Получает список файлов с file_id для администратора.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `limit` - Максимальное количество записей (по умолчанию 50)
///
/// # Returns
///
/// Возвращает `Ok(Vec<SentFile>)` с записями файлов или ошибку базы данных.
/// Возвращает только файлы, у которых есть file_id.
pub fn get_sent_files(conn: &DbConnection, limit: Option<i32>) -> Result<Vec<SentFile>> {
    let limit = limit.unwrap_or(50);
    let mut stmt = conn.prepare(
        "SELECT dh.id, dh.user_id, u.username, dh.url, dh.title, dh.format, dh.downloaded_at, dh.file_id
         FROM download_history dh
         LEFT JOIN users u ON dh.user_id = u.telegram_id
         WHERE dh.file_id IS NOT NULL
         ORDER BY dh.downloaded_at DESC
         LIMIT ?",
    )?;
    let rows = stmt.query_map([limit], |row| {
        Ok(SentFile {
            id: row.get(0)?,
            user_id: row.get(1)?,
            username: row.get(2)?,
            url: row.get(3)?,
            title: row.get(4)?,
            format: row.get(5)?,
            downloaded_at: row.get(6)?,
            file_id: row.get(7)?,
        })
    })?;

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
pub fn delete_download_history_entry(conn: &DbConnection, telegram_id: i64, entry_id: i64) -> Result<bool> {
    let rows_affected = conn.execute(
        "DELETE FROM download_history WHERE id = ?1 AND user_id = ?2",
        [&entry_id as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
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
        "SELECT id, url, title, format, downloaded_at, file_id, author, file_size, duration, video_quality, audio_bitrate,
                bot_api_url, bot_api_is_local, source_id, part_index
         FROM download_history
         WHERE id = ?1 AND user_id = ?2",
    )?;
    let mut rows = stmt.query_map(rusqlite::params![entry_id, telegram_id], |row| {
        Ok(DownloadHistoryEntry {
            id: row.get(0)?,
            url: row.get(1)?,
            title: row.get(2)?,
            format: row.get(3)?,
            downloaded_at: row.get(4)?,
            file_id: row.get(5)?,
            author: row.get(6)?,
            file_size: row.get(7)?,
            duration: row.get(8)?,
            video_quality: row.get(9)?,
            audio_bitrate: row.get(10)?,
            bot_api_url: row.get(11)?,
            bot_api_is_local: row.get(12)?,
            source_id: row.get(13)?,
            part_index: row.get(14)?,
        })
    })?;

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
        [&telegram_id as &dyn rusqlite::ToSql],
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
        [&telegram_id as &dyn rusqlite::ToSql],
        |row| row.get::<_, Option<i64>>(0),
    ) {
        Ok(Some(size)) => size,
        Ok(None) => 0,
        Err(e) => return Err(e),
    };

    // Количество дней активности
    let active_days: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT DATE(downloaded_at)) FROM download_history WHERE user_id = ?",
        [&telegram_id as &dyn rusqlite::ToSql],
        |row| row.get(0),
    )?;

    // Топ-5 исполнителей (парсим из title: "Artist - Song")
    let mut stmt =
        conn.prepare("SELECT title FROM download_history WHERE user_id = ? ORDER BY downloaded_at DESC LIMIT 100")?;
    let rows = stmt.query_map([&telegram_id as &dyn rusqlite::ToSql], |row| row.get::<_, String>(0))?;

    let mut artist_counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
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
    let rows = stmt.query_map([&telegram_id as &dyn rusqlite::ToSql], |row| {
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
    let rows = stmt.query_map([&telegram_id as &dyn rusqlite::ToSql], |row| {
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
    let total_users: i64 = conn.query_row("SELECT COUNT(DISTINCT user_id) FROM download_history", [], |row| {
        row.get(0)
    })?;

    // Общее количество загрузок
    let total_downloads: i64 = conn.query_row("SELECT COUNT(*) FROM download_history", [], |row| row.get(0))?;

    // Топ-10 треков (по title)
    let mut stmt = conn.prepare(
        "SELECT title, COUNT(*) as cnt FROM download_history
         GROUP BY title ORDER BY cnt DESC LIMIT 10",
    )?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;

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
    let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;

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
pub fn get_all_download_history(conn: &DbConnection, telegram_id: i64) -> Result<Vec<DownloadHistoryEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, url, title, format, downloaded_at, file_id, author, file_size, duration, video_quality, audio_bitrate,
                bot_api_url, bot_api_is_local, source_id, part_index
         FROM download_history
         WHERE user_id = ? ORDER BY downloaded_at DESC",
    )?;
    let rows = stmt.query_map(rusqlite::params![telegram_id], |row| {
        Ok(DownloadHistoryEntry {
            id: row.get(0)?,
            url: row.get(1)?,
            title: row.get(2)?,
            format: row.get(3)?,
            downloaded_at: row.get(4)?,
            file_id: row.get(5)?,
            author: row.get(6)?,
            file_size: row.get(7)?,
            duration: row.get(8)?,
            video_quality: row.get(9)?,
            audio_bitrate: row.get(10)?,
            bot_api_url: row.get(11)?,
            bot_api_is_local: row.get(12)?,
            source_id: row.get(13)?,
            part_index: row.get(14)?,
        })
    })?;

    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }
    Ok(entries)
}

/// Получает отфильтрованную историю загрузок для команды /downloads
///
/// Возвращает только файлы с file_id (успешно отправленные) и только mp3/mp4 (исключая субтитры).
/// Поддерживает фильтрацию по типу файла и поиск по названию/автору.
pub fn get_download_history_filtered(
    conn: &DbConnection,
    user_id: i64,
    file_type_filter: Option<&str>,
    search_text: Option<&str>,
) -> Result<Vec<DownloadHistoryEntry>> {
    let mut query = String::from(
        "SELECT id, url, title, format, downloaded_at, file_id, author, file_size,
         duration, video_quality, audio_bitrate, bot_api_url, bot_api_is_local, source_id, part_index
         FROM download_history WHERE user_id = ?",
    );

    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];

    // Only show files with file_id (successfully sent files)
    query.push_str(" AND file_id IS NOT NULL");

    // Only show mp3/mp4 (exclude subtitles)
    query.push_str(" AND (format = 'mp3' OR format = 'mp4')");

    if let Some(ft) = file_type_filter {
        query.push_str(" AND format = ?");
        params.push(Box::new(ft.to_string()));
    }

    if let Some(search) = search_text {
        query.push_str(" AND (title LIKE ? OR author LIKE ?)");
        let search_pattern = format!("%{}%", search);
        params.push(Box::new(search_pattern.clone()));
        params.push(Box::new(search_pattern));
    }

    query.push_str(" ORDER BY downloaded_at DESC");

    let mut stmt = conn.prepare(&query)?;
    let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let downloads = stmt
        .query_map(params_refs.as_slice(), |row| {
            Ok(DownloadHistoryEntry {
                id: row.get(0)?,
                url: row.get(1)?,
                title: row.get(2)?,
                format: row.get(3)?,
                downloaded_at: row.get(4)?,
                file_id: row.get(5)?,
                author: row.get(6)?,
                file_size: row.get(7)?,
                duration: row.get(8)?,
                video_quality: row.get(9)?,
                audio_bitrate: row.get(10)?,
                bot_api_url: row.get(11)?,
                bot_api_is_local: row.get(12)?,
                source_id: row.get(13)?,
                part_index: row.get(14)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(downloads)
}

/// Получает отфильтрованную историю отрезов для команды /downloads
pub fn get_cuts_history_filtered(
    conn: &DbConnection,
    user_id: i64,
    search_text: Option<&str>,
) -> Result<Vec<DownloadHistoryEntry>> {
    let mut query = String::from(
        "SELECT id, original_url, title, output_kind, created_at, file_id, file_size,
         duration, video_quality FROM cuts WHERE user_id = ?",
    );

    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];

    // Only show files with file_id
    query.push_str(" AND file_id IS NOT NULL");

    if let Some(search) = search_text {
        query.push_str(" AND title LIKE ?");
        let search_pattern = format!("%{}%", search);
        params.push(Box::new(search_pattern));
    }

    query.push_str(" ORDER BY created_at DESC");

    let mut stmt = conn.prepare(&query)?;
    let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let cuts = stmt
        .query_map(params_refs.as_slice(), |row| {
            Ok(DownloadHistoryEntry {
                id: row.get(0)?,
                url: row.get(1)?,
                title: row.get(2)?,
                format: String::from("edit"), // Marker for UI
                downloaded_at: row.get(4)?,
                file_id: row.get(5)?,
                author: None,
                file_size: row.get(6)?,
                duration: row.get(7)?,
                video_quality: row.get(8)?,
                audio_bitrate: None,
                bot_api_url: None,
                bot_api_is_local: None,
                source_id: None,
                part_index: None,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(cuts)
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
    let mut stmt = conn.prepare(
        "SELECT
            u.telegram_id,
            u.username,
            COALESCE(s.plan, u.plan) as plan,
            u.download_format,
            u.download_subtitles,
            u.video_quality,
            u.audio_bitrate,
            u.language,
            u.send_as_document,
            u.send_audio_as_document,
            s.expires_at as subscription_expires_at,
            s.telegram_charge_id as telegram_charge_id,
            COALESCE(s.is_recurring, 0) as is_recurring,
            COALESCE(u.burn_subtitles, 0) as burn_subtitles
        FROM users u
        LEFT JOIN subscriptions s ON s.user_id = u.telegram_id
        ORDER BY u.telegram_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(User {
            telegram_id: row.get(0)?,
            username: row.get(1)?,
            plan: row.get(2)?,
            download_format: row.get(3)?,
            download_subtitles: row.get(4)?,
            video_quality: row.get(5).unwrap_or_else(|_| "best".to_string()),
            audio_bitrate: row.get(6).unwrap_or_else(|_| "320k".to_string()),
            language: row.get(7).unwrap_or_else(|_| "ru".to_string()),
            send_as_document: row.get(8).unwrap_or(0),
            send_audio_as_document: row.get(9).unwrap_or(0),
            subscription_expires_at: row.get(10)?,
            telegram_charge_id: row.get(11)?,
            is_recurring: row.get::<_, i32>(12).unwrap_or(0) != 0,
            burn_subtitles: row.get(13).unwrap_or(0),
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
#[allow(clippy::too_many_arguments)]
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
        [
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
pub fn update_task_status(conn: &DbConnection, task_id: &str, status: &str, error_message: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE task_queue SET status = ?1, error_message = ?2, updated_at = CURRENT_TIMESTAMP WHERE id = ?3",
        [
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
        [&error_message as &dyn rusqlite::ToSql, &task_id as &dyn rusqlite::ToSql],
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
    let rows = stmt.query_map([&max_retries as &dyn rusqlite::ToSql], |row| {
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
    let mut rows = stmt.query_map([&task_id as &dyn rusqlite::ToSql], |row| {
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
        [&task_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Помечает задачу как processing
pub fn mark_task_processing(conn: &DbConnection, task_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE task_queue SET status = 'processing', updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
        [&task_id as &dyn rusqlite::ToSql],
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
pub fn update_telegram_charge_id(conn: &DbConnection, telegram_id: i64, charge_id: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE users SET telegram_charge_id = ?1 WHERE telegram_id = ?2",
        [&charge_id as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

// ==================== Audio Effect Sessions ====================

/// Check if user is Premium or VIP
pub fn is_premium_or_vip(conn: &DbConnection, user_id: i64) -> Result<bool> {
    let user = get_user(conn, user_id)?;
    Ok(user.map(|u| u.plan == "premium" || u.plan == "vip").unwrap_or(false))
}

fn ensure_audio_effects_bass_column(conn: &DbConnection) {
    let _ = conn.execute(
        "ALTER TABLE audio_effect_sessions ADD COLUMN bass_gain_db INTEGER DEFAULT 0",
        [],
    );
}

fn ensure_audio_effects_morph_column(conn: &DbConnection) {
    let _ = conn.execute(
        "ALTER TABLE audio_effect_sessions ADD COLUMN morph_profile TEXT DEFAULT 'none'",
        [],
    );
}

/// Create a new audio effect session
pub fn create_audio_effect_session(
    conn: &DbConnection,
    session: &crate::download::audio_effects::AudioEffectSession,
) -> Result<()> {
    ensure_audio_effects_bass_column(conn);
    ensure_audio_effects_morph_column(conn);

    conn.execute(
        "INSERT INTO audio_effect_sessions (
            id, user_id, original_file_path, current_file_path, telegram_file_id,
            original_message_id, title, duration, pitch_semitones, tempo_factor, bass_gain_db, morph_profile,
            version, processing, created_at, expires_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        rusqlite::params![
            session.id,
            session.user_id,
            session.original_file_path,
            session.current_file_path,
            session.telegram_file_id,
            session.original_message_id,
            session.title,
            session.duration,
            session.pitch_semitones,
            session.tempo_factor,
            session.bass_gain_db,
            session.morph_profile.as_str(),
            session.version,
            session.processing as i32,
            session.created_at.to_rfc3339(),
            session.expires_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

/// Get audio effect session by ID
pub fn get_audio_effect_session(
    conn: &DbConnection,
    session_id: &str,
) -> Result<Option<crate::download::audio_effects::AudioEffectSession>> {
    ensure_audio_effects_bass_column(conn);
    ensure_audio_effects_morph_column(conn);
    let mut stmt = conn.prepare(
        "SELECT id, user_id, original_file_path, current_file_path, telegram_file_id,
                original_message_id, title, duration, pitch_semitones, tempo_factor, bass_gain_db, morph_profile,
                version, processing, created_at, expires_at
         FROM audio_effect_sessions WHERE id = ?1",
    )?;

    let result = stmt.query_row([session_id], |row| {
        Ok(crate::download::audio_effects::AudioEffectSession {
            id: row.get(0)?,
            user_id: row.get(1)?,
            original_file_path: row.get(2)?,
            current_file_path: row.get(3)?,
            telegram_file_id: row.get(4)?,
            original_message_id: row.get(5)?,
            title: row.get(6)?,
            duration: row.get(7)?,
            pitch_semitones: row.get(8)?,
            tempo_factor: row.get(9)?,
            bass_gain_db: row.get(10)?,
            morph_profile: crate::download::audio_effects::MorphProfile::parse(row.get::<_, String>(11)?.as_str()),
            version: row.get(12)?,
            processing: row.get::<_, i32>(13)? != 0,
            created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(14)?)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now()),
            expires_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(15)?)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now() + chrono::Duration::hours(24)),
        })
    });

    match result {
        Ok(session) => Ok(Some(session)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Get audio effect session by message ID
pub fn get_audio_effect_session_by_message(
    conn: &DbConnection,
    user_id: i64,
    message_id: i32,
) -> Result<Option<crate::download::audio_effects::AudioEffectSession>> {
    ensure_audio_effects_bass_column(conn);
    ensure_audio_effects_morph_column(conn);
    let mut stmt = conn.prepare(
        "SELECT id, user_id, original_file_path, current_file_path, telegram_file_id,
                original_message_id, title, duration, pitch_semitones, tempo_factor, bass_gain_db, morph_profile,
                version, processing, created_at, expires_at
         FROM audio_effect_sessions WHERE user_id = ?1 AND original_message_id = ?2",
    )?;

    let result = stmt.query_row([user_id, message_id as i64], |row| {
        Ok(crate::download::audio_effects::AudioEffectSession {
            id: row.get(0)?,
            user_id: row.get(1)?,
            original_file_path: row.get(2)?,
            current_file_path: row.get(3)?,
            telegram_file_id: row.get(4)?,
            original_message_id: row.get(5)?,
            title: row.get(6)?,
            duration: row.get(7)?,
            pitch_semitones: row.get(8)?,
            tempo_factor: row.get(9)?,
            bass_gain_db: row.get(10)?,
            morph_profile: crate::download::audio_effects::MorphProfile::parse(row.get::<_, String>(11)?.as_str()),
            version: row.get(12)?,
            processing: row.get::<_, i32>(13)? != 0,
            created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(14)?)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now()),
            expires_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(15)?)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now() + chrono::Duration::hours(24)),
        })
    });

    match result {
        Ok(session) => Ok(Some(session)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Update audio effect session settings
pub fn update_audio_effect_session(
    conn: &DbConnection,
    session_id: &str,
    pitch_semitones: i8,
    tempo_factor: f32,
    bass_gain_db: i8,
    morph_profile: &str,
    current_file_path: &str,
    version: u32,
) -> Result<()> {
    ensure_audio_effects_bass_column(conn);
    ensure_audio_effects_morph_column(conn);
    conn.execute(
        "UPDATE audio_effect_sessions
         SET pitch_semitones = ?1, tempo_factor = ?2, bass_gain_db = ?3, morph_profile = ?4, current_file_path = ?5, version = ?6
         WHERE id = ?7",
        rusqlite::params![
            pitch_semitones,
            tempo_factor,
            bass_gain_db,
            morph_profile,
            current_file_path,
            version,
            session_id
        ],
    )?;
    Ok(())
}

/// Update session Telegram file_id
pub fn update_session_file_id(conn: &DbConnection, session_id: &str, file_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE audio_effect_sessions SET telegram_file_id = ?1 WHERE id = ?2",
        [file_id, session_id],
    )?;
    Ok(())
}

/// Set session processing flag
pub fn set_session_processing(conn: &DbConnection, session_id: &str, processing: bool) -> Result<()> {
    conn.execute(
        "UPDATE audio_effect_sessions SET processing = ?1 WHERE id = ?2",
        rusqlite::params![processing as i32, session_id],
    )?;
    Ok(())
}

/// Delete expired audio effect sessions and return deleted sessions
pub fn delete_expired_audio_sessions(
    conn: &DbConnection,
) -> Result<Vec<crate::download::audio_effects::AudioEffectSession>> {
    ensure_audio_effects_bass_column(conn);
    ensure_audio_effects_morph_column(conn);
    // Get expired sessions
    let mut stmt = conn.prepare(
        "SELECT id, user_id, original_file_path, current_file_path, telegram_file_id,
                original_message_id, title, duration, pitch_semitones, tempo_factor, bass_gain_db, morph_profile,
                version, processing, created_at, expires_at
         FROM audio_effect_sessions WHERE expires_at < ?1",
    )?;

    let now = chrono::Utc::now().to_rfc3339();
    let sessions: Vec<crate::download::audio_effects::AudioEffectSession> = stmt
        .query_map([now], |row| {
            Ok(crate::download::audio_effects::AudioEffectSession {
                id: row.get(0)?,
                user_id: row.get(1)?,
                original_file_path: row.get(2)?,
                current_file_path: row.get(3)?,
                telegram_file_id: row.get(4)?,
                original_message_id: row.get(5)?,
                title: row.get(6)?,
                duration: row.get(7)?,
                pitch_semitones: row.get(8)?,
                tempo_factor: row.get(9)?,
                bass_gain_db: row.get(10)?,
                morph_profile: crate::download::audio_effects::MorphProfile::parse(row.get::<_, String>(11)?.as_str()),
                version: row.get(12)?,
                processing: row.get::<_, i32>(13)? != 0,
                created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(14)?)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now()),
                expires_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(15)?)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now() + chrono::Duration::hours(24)),
            })
        })?
        .collect::<Result<Vec<_>>>()?;

    // Delete expired sessions
    let session_ids: Vec<String> = sessions.iter().map(|s| s.id.clone()).collect();
    for session_id in session_ids {
        conn.execute("DELETE FROM audio_effect_sessions WHERE id = ?1", [&session_id])?;
    }

    Ok(sessions)
}

/// Delete specific audio effect session
pub fn delete_audio_effect_session(conn: &DbConnection, session_id: &str) -> Result<()> {
    conn.execute("DELETE FROM audio_effect_sessions WHERE id = ?1", [session_id])?;
    Ok(())
}

// ==================== Audio Cut Sessions ====================

#[derive(Debug, Clone)]
pub struct AudioCutSession {
    pub id: String,
    pub user_id: i64,
    pub audio_session_id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

pub fn upsert_audio_cut_session(conn: &DbConnection, session: &AudioCutSession) -> Result<()> {
    conn.execute("DELETE FROM audio_cut_sessions WHERE user_id = ?1", [session.user_id])?;
    conn.execute(
        "INSERT INTO audio_cut_sessions (
            id, user_id, audio_session_id, created_at, expires_at
         ) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            session.id,
            session.user_id,
            session.audio_session_id,
            session.created_at.to_rfc3339(),
            session.expires_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn get_active_audio_cut_session(conn: &DbConnection, user_id: i64) -> Result<Option<AudioCutSession>> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut stmt = conn.prepare(
        "SELECT id, user_id, audio_session_id, created_at, expires_at
         FROM audio_cut_sessions
         WHERE user_id = ?1 AND expires_at > ?2
         ORDER BY created_at DESC
         LIMIT 1",
    )?;
    let mut rows = stmt.query(rusqlite::params![user_id, now])?;
    if let Some(row) = rows.next()? {
        let created_at: String = row.get(3)?;
        let expires_at: String = row.get(4)?;
        Ok(Some(AudioCutSession {
            id: row.get(0)?,
            user_id: row.get(1)?,
            audio_session_id: row.get(2)?,
            created_at: chrono::DateTime::parse_from_rfc3339(&created_at)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now()),
            expires_at: chrono::DateTime::parse_from_rfc3339(&expires_at)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now() + chrono::Duration::minutes(10)),
        }))
    } else {
        Ok(None)
    }
}

pub fn delete_audio_cut_session_by_user(conn: &DbConnection, user_id: i64) -> Result<()> {
    conn.execute("DELETE FROM audio_cut_sessions WHERE user_id = ?1", [user_id])?;
    Ok(())
}

// ==================== Video Clip Sessions ====================

#[derive(Debug, Clone)]
pub struct VideoClipSession {
    pub id: String,
    pub user_id: i64,
    pub source_download_id: i64,
    pub source_kind: String,
    pub source_id: i64,
    pub original_url: String,
    pub output_kind: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

pub fn upsert_video_clip_session(conn: &DbConnection, session: &VideoClipSession) -> Result<()> {
    conn.execute("DELETE FROM video_clip_sessions WHERE user_id = ?1", [session.user_id])?;
    conn.execute(
        "INSERT INTO video_clip_sessions (
            id, user_id, source_download_id, source_kind, source_id, original_url, output_kind, created_at, expires_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            session.id,
            session.user_id,
            session.source_download_id,
            session.source_kind,
            session.source_id,
            session.original_url,
            session.output_kind,
            session.created_at.to_rfc3339(),
            session.expires_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn get_active_video_clip_session(conn: &DbConnection, user_id: i64) -> Result<Option<VideoClipSession>> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut stmt = conn.prepare(
        "SELECT id, user_id, source_download_id, source_kind, source_id, original_url, output_kind, created_at, expires_at
         FROM video_clip_sessions
         WHERE user_id = ?1 AND expires_at > ?2
         ORDER BY created_at DESC
         LIMIT 1",
    )?;
    let mut rows = stmt.query(rusqlite::params![user_id, now])?;
    if let Some(row) = rows.next()? {
        let source_download_id: i64 = row.get(2)?;
        let source_kind: Option<String> = row.get(3)?;
        let source_id: Option<i64> = row.get(4)?;
        let original_url: Option<String> = row.get(5)?;
        let output_kind: Option<String> = row.get(6)?;
        let created_at: String = row.get(7)?;
        let expires_at: String = row.get(8)?;
        let resolved_source_kind = source_kind.unwrap_or_else(|| "download".to_string());
        let resolved_source_id = source_id.unwrap_or(source_download_id);
        let resolved_original_url = original_url.unwrap_or_default();
        let resolved_output_kind = output_kind.unwrap_or_else(|| "cut".to_string());
        Ok(Some(VideoClipSession {
            id: row.get(0)?,
            user_id: row.get(1)?,
            source_download_id,
            source_kind: resolved_source_kind,
            source_id: resolved_source_id,
            original_url: resolved_original_url,
            output_kind: resolved_output_kind,
            created_at: chrono::DateTime::parse_from_rfc3339(&created_at)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now()),
            expires_at: chrono::DateTime::parse_from_rfc3339(&expires_at)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now() + chrono::Duration::minutes(10)),
        }))
    } else {
        Ok(None)
    }
}

pub fn delete_video_clip_session_by_user(conn: &DbConnection, user_id: i64) -> Result<()> {
    conn.execute("DELETE FROM video_clip_sessions WHERE user_id = ?1", [user_id])?;
    Ok(())
}

// ==================== Cuts ====================

#[derive(Debug, Clone)]
pub struct CutEntry {
    pub id: i64,
    pub user_id: i64,
    pub original_url: String,
    pub source_kind: String,
    pub source_id: i64,
    pub output_kind: String,
    pub segments_json: String,
    pub segments_text: String,
    pub title: String,
    pub created_at: String,
    pub file_id: Option<String>,
    pub file_size: Option<i64>,
    pub duration: Option<i64>,
    pub video_quality: Option<String>,
}

pub fn create_cut(
    conn: &DbConnection,
    user_id: i64,
    original_url: &str,
    source_kind: &str,
    source_id: i64,
    output_kind: &str,
    segments_json: &str,
    segments_text: &str,
    title: &str,
    file_id: Option<&str>,
    file_size: Option<i64>,
    duration: Option<i64>,
    video_quality: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO cuts (
            user_id, original_url, source_kind, source_id, output_kind, segments_json, segments_text,
            title, file_id, file_size, duration, video_quality
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        rusqlite::params![
            user_id,
            original_url,
            source_kind,
            source_id,
            output_kind,
            segments_json,
            segments_text,
            title,
            file_id,
            file_size,
            duration,
            video_quality,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_cuts(conn: &DbConnection, user_id: i64, limit: Option<i32>) -> Result<Vec<CutEntry>> {
    let limit = limit.unwrap_or(50);
    let mut stmt = conn.prepare(
        "SELECT id, user_id, original_url, source_kind, source_id, output_kind, segments_json, segments_text,
                title, created_at, file_id, file_size, duration, video_quality
         FROM cuts
         WHERE user_id = ?1
         ORDER BY created_at DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![user_id, limit], |row| {
        Ok(CutEntry {
            id: row.get(0)?,
            user_id: row.get(1)?,
            original_url: row.get(2)?,
            source_kind: row.get(3)?,
            source_id: row.get(4)?,
            output_kind: row.get(5)?,
            segments_json: row.get(6)?,
            segments_text: row.get(7)?,
            title: row.get(8)?,
            created_at: row.get(9)?,
            file_id: row.get(10)?,
            file_size: row.get(11)?,
            duration: row.get(12)?,
            video_quality: row.get(13)?,
        })
    })?;

    rows.collect::<Result<Vec<_>>>()
}

pub fn get_cuts_count(conn: &DbConnection, user_id: i64) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM cuts WHERE user_id = ?1",
        rusqlite::params![user_id],
        |row| row.get(0),
    )
}

pub fn get_cuts_page(conn: &DbConnection, user_id: i64, limit: i64, offset: i64) -> Result<Vec<CutEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, original_url, source_kind, source_id, output_kind, segments_json, segments_text,
                title, created_at, file_id, file_size, duration, video_quality
         FROM cuts
         WHERE user_id = ?1
         ORDER BY created_at DESC
         LIMIT ?2 OFFSET ?3",
    )?;
    let rows = stmt.query_map(rusqlite::params![user_id, limit, offset], |row| {
        Ok(CutEntry {
            id: row.get(0)?,
            user_id: row.get(1)?,
            original_url: row.get(2)?,
            source_kind: row.get(3)?,
            source_id: row.get(4)?,
            output_kind: row.get(5)?,
            segments_json: row.get(6)?,
            segments_text: row.get(7)?,
            title: row.get(8)?,
            created_at: row.get(9)?,
            file_id: row.get(10)?,
            file_size: row.get(11)?,
            duration: row.get(12)?,
            video_quality: row.get(13)?,
        })
    })?;

    rows.collect::<Result<Vec<_>>>()
}

pub fn get_cut_entry(conn: &DbConnection, user_id: i64, cut_id: i64) -> Result<Option<CutEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, original_url, source_kind, source_id, output_kind, segments_json, segments_text,
                title, created_at, file_id, file_size, duration, video_quality
         FROM cuts
         WHERE id = ?1 AND user_id = ?2",
    )?;
    let mut rows = stmt.query(rusqlite::params![cut_id, user_id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(CutEntry {
            id: row.get(0)?,
            user_id: row.get(1)?,
            original_url: row.get(2)?,
            source_kind: row.get(3)?,
            source_id: row.get(4)?,
            output_kind: row.get(5)?,
            segments_json: row.get(6)?,
            segments_text: row.get(7)?,
            title: row.get(8)?,
            created_at: row.get(9)?,
            file_id: row.get(10)?,
            file_size: row.get(11)?,
            duration: row.get(12)?,
            video_quality: row.get(13)?,
        }))
    } else {
        Ok(None)
    }
}

// ==================== Subscription Management ====================

/// Получает запись подписки пользователя из таблицы subscriptions.
pub fn get_subscription(conn: &DbConnection, telegram_id: i64) -> Result<Option<Subscription>> {
    let mut stmt = conn.prepare(
        "SELECT user_id, plan, expires_at, telegram_charge_id, is_recurring
         FROM subscriptions
         WHERE user_id = ?1",
    )?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(Some(Subscription {
            user_id: row.get(0)?,
            plan: row.get(1)?,
            expires_at: row.get::<_, Option<String>>(2)?,
            telegram_charge_id: row.get::<_, Option<String>>(3)?,
            is_recurring: row.get::<_, i32>(4).unwrap_or(0) != 0,
        }))
    } else {
        Ok(None)
    }
}

/// Обновляет данные подписки пользователя при успешном платеже.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
/// * `plan` - Новый план пользователя (например, "premium", "vip")
/// * `charge_id` - Telegram payment charge ID из успешного платежа
/// * `subscription_expires_at` - Дата истечения подписки (Unix timestamp или ISO 8601 строка)
/// * `is_recurring` - Флаг рекуррентной подписки (автопродление)
///
/// # Returns
///
/// Возвращает `Ok(())` при успехе или ошибку базы данных.
pub fn update_subscription_data(
    conn: &DbConnection,
    telegram_id: i64,
    plan: &str,
    charge_id: &str,
    subscription_expires_at: &str,
    is_recurring: bool,
) -> Result<()> {
    conn.execute(
        "INSERT INTO subscriptions (user_id, plan, expires_at, telegram_charge_id, is_recurring)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(user_id) DO UPDATE SET
            plan = excluded.plan,
            expires_at = excluded.expires_at,
            telegram_charge_id = excluded.telegram_charge_id,
            is_recurring = excluded.is_recurring,
            updated_at = CURRENT_TIMESTAMP",
        [
            &telegram_id as &dyn rusqlite::ToSql,
            &plan as &dyn rusqlite::ToSql,
            &subscription_expires_at as &dyn rusqlite::ToSql,
            &charge_id as &dyn rusqlite::ToSql,
            &(if is_recurring { 1 } else { 0 }) as &dyn rusqlite::ToSql,
        ],
    )?;
    conn.execute(
        "UPDATE users SET plan = ?1 WHERE telegram_id = ?2",
        [&plan as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Проверяет, активна ли подписка пользователя.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
///
/// # Returns
///
/// Возвращает `Ok(true)` если подписка активна, `Ok(false)` если нет или истекла.
pub fn is_subscription_active(conn: &DbConnection, telegram_id: i64) -> Result<bool> {
    let subscription = get_subscription(conn, telegram_id)?;

    let Some(subscription) = subscription else {
        return Ok(false);
    };

    if subscription.plan == "free" {
        return Ok(false);
    }

    if let Some(expires_at) = subscription.expires_at {
        let mut stmt = conn.prepare("SELECT datetime('now') < datetime(?1)")?;
        let is_active: bool = stmt.query_row([&expires_at], |row| row.get(0))?;
        Ok(is_active)
    } else {
        Ok(true)
    }
}

/// Отменяет подписку пользователя (сбрасывает флаг is_recurring).
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `telegram_id` - Telegram ID пользователя
///
/// # Returns
///
/// Возвращает `Ok(())` при успехе или ошибку базы данных.
///
/// # Note
///
/// Эта функция только убирает флаг автопродления. Пользователь сохраняет
/// доступ до даты истечения подписки (subscription_expires_at).
pub fn cancel_subscription(conn: &DbConnection, telegram_id: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO subscriptions (user_id, plan, is_recurring)
         VALUES (?1, 'free', 0)
         ON CONFLICT(user_id) DO UPDATE SET
            is_recurring = 0,
            updated_at = CURRENT_TIMESTAMP",
        [&telegram_id as &dyn rusqlite::ToSql],
    )?;
    conn.execute(
        "UPDATE users SET plan = 'free' WHERE telegram_id = ?1",
        [&telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Получает информацию о статусе подписки пользователя.
///
/// # Returns
///
/// Возвращает кортеж: (plan, expires_at, is_recurring, is_active)
pub type SubscriptionStatus = (String, Option<String>, bool, bool);

pub fn get_subscription_status(conn: &DbConnection, telegram_id: i64) -> Result<Option<SubscriptionStatus>> {
    let subscription = get_subscription(conn, telegram_id)?;

    if let Some(subscription) = subscription {
        let is_active = is_subscription_active(conn, telegram_id)?;
        Ok(Some((
            subscription.plan,
            subscription.expires_at,
            subscription.is_recurring,
            is_active,
        )))
    } else {
        Ok(None)
    }
}

/// Сохраняет информацию о платеже (charge) в базу данных.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `user_id` - Telegram ID пользователя
/// * `plan` - План подписки ("premium" или "vip")
/// * `telegram_charge_id` - ID платежа из Telegram
/// * `provider_charge_id` - ID платежа от провайдера (опционально)
/// * `currency` - Валюта платежа (например, "XTR" для Stars)
/// * `total_amount` - Общая сумма платежа
/// * `invoice_payload` - Payload инвойса
/// * `is_recurring` - Флаг рекуррентной подписки
/// * `is_first_recurring` - Флаг первого платежа рекуррентной подписки
/// * `subscription_expiration_date` - Дата истечения подписки
///
/// # Returns
///
/// Возвращает `Result<i64>` с ID созданной записи или ошибку.
pub fn save_charge(
    conn: &DbConnection,
    user_id: i64,
    plan: &str,
    telegram_charge_id: &str,
    provider_charge_id: Option<&str>,
    currency: &str,
    total_amount: i64,
    invoice_payload: &str,
    is_recurring: bool,
    is_first_recurring: bool,
    subscription_expiration_date: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO charges (
            user_id, plan, telegram_charge_id, provider_charge_id, currency,
            total_amount, invoice_payload, is_recurring, is_first_recurring,
            subscription_expiration_date
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            user_id,
            plan,
            telegram_charge_id,
            provider_charge_id,
            currency,
            total_amount,
            invoice_payload,
            is_recurring as i32,
            is_first_recurring as i32,
            subscription_expiration_date,
        ],
    )?;

    Ok(conn.last_insert_rowid())
}

/// Получает все charges для конкретного пользователя.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `user_id` - Telegram ID пользователя
///
/// # Returns
///
/// Возвращает `Result<Vec<Charge>>` со списком всех платежей пользователя.
pub fn get_user_charges(conn: &DbConnection, user_id: i64) -> Result<Vec<Charge>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, plan, telegram_charge_id, provider_charge_id, currency,
                total_amount, invoice_payload, is_recurring, is_first_recurring,
                subscription_expiration_date, payment_date, created_at
         FROM charges
         WHERE user_id = ?1
         ORDER BY payment_date DESC",
    )?;

    let charges = stmt.query_map([user_id], |row| {
        Ok(Charge {
            id: row.get(0)?,
            user_id: row.get(1)?,
            plan: row.get(2)?,
            telegram_charge_id: row.get(3)?,
            provider_charge_id: row.get(4)?,
            currency: row.get(5)?,
            total_amount: row.get(6)?,
            invoice_payload: row.get(7)?,
            is_recurring: row.get::<_, i32>(8)? != 0,
            is_first_recurring: row.get::<_, i32>(9)? != 0,
            subscription_expiration_date: row.get(10)?,
            payment_date: row.get(11)?,
            created_at: row.get(12)?,
        })
    })?;

    charges.collect()
}

/// Получает все charges из базы данных с возможностью фильтрации и пагинации.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `plan_filter` - Фильтр по плану (None = все планы)
/// * `limit` - Максимальное количество записей (None = все)
/// * `offset` - Смещение для пагинации
///
/// # Returns
///
/// Возвращает `Result<Vec<Charge>>` со списком всех платежей.
pub fn get_all_charges(
    conn: &DbConnection,
    plan_filter: Option<&str>,
    limit: Option<i64>,
    offset: i64,
) -> Result<Vec<Charge>> {
    let query = if let Some(plan) = plan_filter {
        format!(
            "SELECT id, user_id, plan, telegram_charge_id, provider_charge_id, currency,
                    total_amount, invoice_payload, is_recurring, is_first_recurring,
                    subscription_expiration_date, payment_date, created_at
             FROM charges
             WHERE plan = '{}'
             ORDER BY payment_date DESC
             LIMIT {} OFFSET {}",
            plan,
            limit.unwrap_or(-1),
            offset
        )
    } else {
        format!(
            "SELECT id, user_id, plan, telegram_charge_id, provider_charge_id, currency,
                    total_amount, invoice_payload, is_recurring, is_first_recurring,
                    subscription_expiration_date, payment_date, created_at
             FROM charges
             ORDER BY payment_date DESC
             LIMIT {} OFFSET {}",
            limit.unwrap_or(-1),
            offset
        )
    };

    let mut stmt = conn.prepare(&query)?;

    let charges = stmt.query_map([], |row| {
        Ok(Charge {
            id: row.get(0)?,
            user_id: row.get(1)?,
            plan: row.get(2)?,
            telegram_charge_id: row.get(3)?,
            provider_charge_id: row.get(4)?,
            currency: row.get(5)?,
            total_amount: row.get(6)?,
            invoice_payload: row.get(7)?,
            is_recurring: row.get::<_, i32>(8)? != 0,
            is_first_recurring: row.get::<_, i32>(9)? != 0,
            subscription_expiration_date: row.get(10)?,
            payment_date: row.get(11)?,
            created_at: row.get(12)?,
        })
    })?;

    charges.collect()
}

/// Получает статистику по платежам.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
///
/// # Returns
///
/// Возвращает кортеж (total_charges, total_amount, premium_count, vip_count, recurring_count).
pub fn get_charges_stats(conn: &DbConnection) -> Result<(i64, i64, i64, i64, i64)> {
    let mut stmt = conn.prepare(
        "SELECT
            COUNT(*) as total_charges,
            SUM(total_amount) as total_amount,
            SUM(CASE WHEN plan = 'premium' THEN 1 ELSE 0 END) as premium_count,
            SUM(CASE WHEN plan = 'vip' THEN 1 ELSE 0 END) as vip_count,
            SUM(CASE WHEN is_recurring = 1 THEN 1 ELSE 0 END) as recurring_count
         FROM charges",
    )?;

    stmt.query_row([], |row| {
        Ok((
            row.get(0)?,
            row.get::<_, Option<i64>>(1)?.unwrap_or(0),
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        ))
    })
}

/// Сохраняет отзыв пользователя в базу данных.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `user_id` - Telegram ID пользователя
/// * `username` - Username пользователя (опционально)
/// * `first_name` - Имя пользователя
/// * `message` - Текст отзыва
///
/// # Returns
///
/// Возвращает `Result<i64>` с ID созданной записи или ошибку.
pub fn save_feedback(
    conn: &DbConnection,
    user_id: i64,
    username: Option<&str>,
    first_name: &str,
    message: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO feedback_messages (user_id, username, first_name, message, status)
         VALUES (?1, ?2, ?3, ?4, 'new')",
        rusqlite::params![user_id, username, first_name, message],
    )?;

    Ok(conn.last_insert_rowid())
}

/// Получает все отзывы с возможностью фильтрации по статусу.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `status_filter` - Фильтр по статусу ("new", "read", "replied", None = все)
/// * `limit` - Максимальное количество записей (None = все)
/// * `offset` - Смещение для пагинации
///
/// # Returns
///
/// Возвращает `Result<Vec<FeedbackMessage>>` со списком отзывов.
pub fn get_feedback_messages(
    conn: &DbConnection,
    status_filter: Option<&str>,
    limit: Option<i64>,
    offset: i64,
) -> Result<Vec<FeedbackMessage>> {
    let query = if let Some(status) = status_filter {
        format!(
            "SELECT id, user_id, username, first_name, message, status,
                    admin_reply, created_at, replied_at
             FROM feedback_messages
             WHERE status = '{}'
             ORDER BY created_at DESC
             LIMIT {} OFFSET {}",
            status,
            limit.unwrap_or(-1),
            offset
        )
    } else {
        format!(
            "SELECT id, user_id, username, first_name, message, status,
                    admin_reply, created_at, replied_at
             FROM feedback_messages
             ORDER BY created_at DESC
             LIMIT {} OFFSET {}",
            limit.unwrap_or(-1),
            offset
        )
    };

    let mut stmt = conn.prepare(&query)?;

    let messages = stmt.query_map([], |row| {
        Ok(FeedbackMessage {
            id: row.get(0)?,
            user_id: row.get(1)?,
            username: row.get(2)?,
            first_name: row.get(3)?,
            message: row.get(4)?,
            status: row.get(5)?,
            admin_reply: row.get(6)?,
            created_at: row.get(7)?,
            replied_at: row.get(8)?,
        })
    })?;

    messages.collect()
}

/// Получает отзывы конкретного пользователя.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `user_id` - Telegram ID пользователя
///
/// # Returns
///
/// Возвращает `Result<Vec<FeedbackMessage>>` со списком отзывов пользователя.
pub fn get_user_feedback(conn: &DbConnection, user_id: i64) -> Result<Vec<FeedbackMessage>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, username, first_name, message, status,
                admin_reply, created_at, replied_at
         FROM feedback_messages
         WHERE user_id = ?1
         ORDER BY created_at DESC",
    )?;

    let messages = stmt.query_map([user_id], |row| {
        Ok(FeedbackMessage {
            id: row.get(0)?,
            user_id: row.get(1)?,
            username: row.get(2)?,
            first_name: row.get(3)?,
            message: row.get(4)?,
            status: row.get(5)?,
            admin_reply: row.get(6)?,
            created_at: row.get(7)?,
            replied_at: row.get(8)?,
        })
    })?;

    messages.collect()
}

/// Обновляет статус отзыва.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `feedback_id` - ID отзыва
/// * `status` - Новый статус ("new", "read", "replied")
///
/// # Returns
///
/// Возвращает `Result<()>` или ошибку.
pub fn update_feedback_status(conn: &DbConnection, feedback_id: i64, status: &str) -> Result<()> {
    conn.execute(
        "UPDATE feedback_messages SET status = ?1 WHERE id = ?2",
        rusqlite::params![status, feedback_id],
    )?;
    Ok(())
}

/// Добавляет ответ администратора на отзыв.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
/// * `feedback_id` - ID отзыва
/// * `reply` - Текст ответа
///
/// # Returns
///
/// Возвращает `Result<()>` или ошибку.
pub fn add_feedback_reply(conn: &DbConnection, feedback_id: i64, reply: &str) -> Result<()> {
    conn.execute(
        "UPDATE feedback_messages
         SET admin_reply = ?1, status = 'replied', replied_at = CURRENT_TIMESTAMP
         WHERE id = ?2",
        rusqlite::params![reply, feedback_id],
    )?;
    Ok(())
}

/// Получает статистику по отзывам.
///
/// # Arguments
///
/// * `conn` - Соединение с базой данных
///
/// # Returns
///
/// Возвращает кортеж (total_feedback, new_count, read_count, replied_count).
pub fn get_feedback_stats(conn: &DbConnection) -> Result<(i64, i64, i64, i64)> {
    let mut stmt = conn.prepare(
        "SELECT
            COUNT(*) as total_feedback,
            SUM(CASE WHEN status = 'new' THEN 1 ELSE 0 END) as new_count,
            SUM(CASE WHEN status = 'read' THEN 1 ELSE 0 END) as read_count,
            SUM(CASE WHEN status = 'replied' THEN 1 ELSE 0 END) as replied_count
         FROM feedback_messages",
    )?;

    stmt.query_row([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
}
// ==================== Cookies Upload Sessions ====================

#[derive(Debug, Clone)]
pub struct CookiesUploadSession {
    pub id: String,
    pub user_id: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

pub fn upsert_cookies_upload_session(conn: &DbConnection, session: &CookiesUploadSession) -> Result<()> {
    conn.execute(
        "DELETE FROM cookies_upload_sessions WHERE user_id = ?1",
        [session.user_id],
    )?;
    conn.execute(
        "INSERT INTO cookies_upload_sessions (id, user_id, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            session.id,
            session.user_id,
            session.created_at.to_rfc3339(),
            session.expires_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn get_active_cookies_upload_session(conn: &DbConnection, user_id: i64) -> Result<Option<CookiesUploadSession>> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut stmt = conn.prepare(
        "SELECT id, user_id, created_at, expires_at
         FROM cookies_upload_sessions
         WHERE user_id = ?1 AND expires_at > ?2
         ORDER BY created_at DESC
         LIMIT 1",
    )?;
    let mut rows = stmt.query(rusqlite::params![user_id, now])?;
    if let Some(row) = rows.next()? {
        let created_at: String = row.get(2)?;
        let expires_at: String = row.get(3)?;
        Ok(Some(CookiesUploadSession {
            id: row.get(0)?,
            user_id: row.get(1)?,
            created_at: chrono::DateTime::parse_from_rfc3339(&created_at)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now()),
            expires_at: chrono::DateTime::parse_from_rfc3339(&expires_at)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now() + chrono::Duration::minutes(10)),
        }))
    } else {
        Ok(None)
    }
}

pub fn delete_cookies_upload_session_by_user(conn: &DbConnection, user_id: i64) -> Result<()> {
    conn.execute("DELETE FROM cookies_upload_sessions WHERE user_id = ?1", [user_id])?;
    Ok(())
}
