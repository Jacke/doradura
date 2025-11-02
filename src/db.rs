use rusqlite::{Connection, Result};
use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;

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
        "INSERT INTO users (telegram_id, username, download_format, download_subtitles) VALUES (?1, ?2, 'mp3', 0)",
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
    let mut stmt = conn.prepare("SELECT telegram_id, username, plan, download_format, download_subtitles FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query(&[&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        let telegram_id: i64 = row.get(0)?;
        let username: Option<String> = row.get(1)?;
        let plan: String = row.get(2)?;
        let download_format: String = row.get(3)?;
        let download_subtitles: i32 = row.get(4)?;

        Ok(Some(User {
            telegram_id,
            username,
            plan,
            download_format,
            download_subtitles,
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
        &[&plan as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
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
        &[&user_id as &dyn rusqlite::ToSql, &request_text as &dyn rusqlite::ToSql],
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
        &[&format as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
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
pub fn set_user_download_subtitles(conn: &DbConnection, telegram_id: i64, enabled: bool) -> Result<()> {
    let value = if enabled { 1 } else { 0 };
    conn.execute(
        "UPDATE users SET download_subtitles = ?1 WHERE telegram_id = ?2",
        &[&value as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
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
         WHERE user_id = ? ORDER BY downloaded_at DESC LIMIT ?"
    )?;
    let rows = stmt.query_map(
        &[&telegram_id as &dyn rusqlite::ToSql, &limit as &dyn rusqlite::ToSql],
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
         WHERE id = ?1 AND user_id = ?2"
    )?;
    let mut rows = stmt.query_map(
        &[&entry_id as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
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
