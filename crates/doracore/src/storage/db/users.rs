//! User CRUD, settings, plan management, and subtitle style operations.

use super::DbConnection;
use crate::core::types::Plan;
use rusqlite::Result;

/// Structure representing a user in the database.
pub struct User {
    /// Telegram ID of the user
    pub telegram_id: i64,
    /// Telegram username, if available
    pub username: Option<String>,
    /// User plan
    pub plan: Plan,
    /// Preferred download format: "mp3", "mp4", "srt", "txt"
    pub download_format: String,
    /// Subtitle download flag (0 = disabled, 1 = enabled)
    pub download_subtitles: i32,
    /// Video quality: "best", "1080p", "720p", "480p", "360p"
    pub video_quality: String,
    /// Audio bitrate: "128k", "192k", "256k", "320k"
    pub audio_bitrate: String,
    /// Video send type: 0 = Media (send_video), 1 = Document (send_document)
    pub send_as_document: i32,
    /// Audio send type: 0 = Media (send_audio), 1 = Document (send_document)
    pub send_audio_as_document: i32,
    /// Subscription expiry date (from subscriptions table)
    pub subscription_expires_at: Option<String>,
    /// Telegram payment charge ID (from subscriptions table)
    pub telegram_charge_id: Option<String>,
    /// Preferred user language (IETF tag, e.g. "ru", "en-US")
    pub language: String,
    /// Recurring subscription flag (auto-renewal) from subscriptions table
    pub is_recurring: bool,
    /// Subtitle burn-in flag for video (0 = disabled, 1 = enabled)
    pub burn_subtitles: i32,
    /// Progress bar style: "classic", "gradient", "emoji", "dots", "runner", "rpg", "fire", "moon"
    pub progress_bar_style: String,
    /// Whether the user is blocked by admin
    pub is_blocked: bool,
    /// Experimental features flag (0 = disabled, 1 = enabled)
    pub experimental_features: i32,
}

impl User {
    /// Returns the Telegram ID of the user.
    ///
    /// # Returns
    ///
    /// Telegram ID of the user.
    pub fn telegram_id(&self) -> i64 {
        self.telegram_id
    }

    /// Returns the preferred download format of the user.
    ///
    /// # Returns
    ///
    /// Download format: "mp3", "mp4", "srt", "txt"
    pub fn download_format(&self) -> &str {
        &self.download_format
    }
}

/// Aggregated user counts.
#[derive(Debug, Default)]
pub struct UserCounts {
    pub total: usize,
    pub free: usize,
    pub premium: usize,
    pub vip: usize,
    pub blocked: usize,
}

// ==================== Subtitle Style Settings ====================

/// User's subtitle style preferences for burned subtitles.
#[derive(Debug, Clone)]
pub struct SubtitleStyle {
    pub font_size: String,
    pub text_color: String,
    pub outline_color: String,
    pub outline_width: i32,
    pub shadow: i32,
    pub position: String,
    /// Bottom margin in pixels (keeps subs inside circle mask)
    pub margin_v: i32,
    /// Horizontal margin in pixels (MarginL + MarginR, keeps subs inside circle)
    pub margin_h: i32,
    /// Bold text (1=bold, 0=normal)
    pub bold: i32,
}

impl Default for SubtitleStyle {
    fn default() -> Self {
        Self {
            font_size: "medium".to_string(),
            text_color: "white".to_string(),
            outline_color: "black".to_string(),
            outline_width: 2,
            shadow: 1,
            position: "bottom".to_string(),
            margin_v: 0,
            margin_h: 0,
            bold: 0,
        }
    }
}

impl SubtitleStyle {
    /// Style optimized for 640x640 circle video notes.
    /// Subtitles are burned AFTER scale+crop, so these values are in 640x640 coordinates.
    /// Small font, bold, thick outline, with margins to keep text inside circular mask.
    ///
    /// Circle geometry at MarginV=55: chord width ~ 390px -> MarginH=125 each side
    /// keeps text safely within the visible circle area.
    pub fn circle_default() -> Self {
        Self {
            font_size: "small".to_string(), // 16px -- compact for 640x640
            text_color: "white".to_string(),
            outline_color: "black".to_string(),
            outline_width: 2,
            shadow: 0,
            position: "bottom".to_string(),
            margin_v: 55,  // lift above bottom circle cutoff (640x640 coords)
            margin_h: 125, // keep text within circle chord at this height
            bold: 1,
        }
    }

    /// Builds the ffmpeg force_style string from subtitle settings.
    pub fn to_force_style(&self) -> String {
        let font_size = match self.font_size.as_str() {
            "small" => 16,
            "medium" => 24,
            "large" => 32,
            "xlarge" => 40,
            _ => 24,
        };

        let primary_colour = match self.text_color.as_str() {
            "white" => "&H00FFFFFF",
            "yellow" => "&H0000FFFF",
            "cyan" => "&H00FFFF00",
            "green" => "&H0000FF00",
            _ => "&H00FFFFFF",
        };

        let outline_colour = match self.outline_color.as_str() {
            "black" => "&H00000000",
            "dark_gray" => "&H00404040",
            "none" => "&HFF000000",
            _ => "&H00000000",
        };

        // ASS Alignment: bottom-center=2, top-center=8
        let alignment = match self.position.as_str() {
            "top" => 8,
            _ => 2,
        };

        let mut style = format!(
            "FontName=DejaVu Sans,FontSize={},PrimaryColour={},OutlineColour={},Outline={},Shadow={},Alignment={}",
            font_size, primary_colour, outline_colour, self.outline_width, self.shadow, alignment
        );

        if self.margin_v > 0 {
            style.push_str(&format!(",MarginV={}", self.margin_v));
        }
        if self.margin_h > 0 {
            style.push_str(&format!(",MarginL={},MarginR={}", self.margin_h, self.margin_h));
        }
        if self.bold != 0 {
            style.push_str(&format!(",Bold={}", self.bold));
        }

        style
    }
}

// ==================== User CRUD ====================

/// Creates a new user in the database.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `username` - Username (optional)
///
/// # Returns
///
/// Returns `Ok(())` on success or a database error.
///
/// # Errors
///
/// Returns an error if a user with the given ID already exists or a DB error occurred.
pub fn create_user(conn: &DbConnection, telegram_id: i64, username: Option<String>) -> Result<()> {
    // Use a transaction to ensure both inserts succeed or fail together
    conn.execute_batch("BEGIN IMMEDIATE")?;

    let result = (|| {
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
    })();

    match result {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(e) => {
            if let Err(rb_err) = conn.execute_batch("ROLLBACK") {
                log::error!("ROLLBACK failed: {}", rb_err);
            }
            Err(e)
        }
    }
}

/// Creates a new user in the database with the specified language.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `username` - Username (optional)
/// * `language` - Language code (e.g. "ru", "en", "fr", "de")
///
/// # Returns
///
/// Returns `Ok(())` on success or a database error.
///
/// # Errors
///
/// Returns an error if a user with the given ID already exists or a DB error occurred.
pub fn create_user_with_language(
    conn: &DbConnection,
    telegram_id: i64,
    username: Option<String>,
    language: &str,
) -> Result<()> {
    // Use a transaction to ensure both inserts succeed or fail together
    conn.execute_batch("BEGIN IMMEDIATE")?;

    let result = (|| {
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
    })();

    match result {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(e) => {
            if let Err(rb_err) = conn.execute_batch("ROLLBACK") {
                log::error!("ROLLBACK failed: {}", rb_err);
            }
            Err(e)
        }
    }
}

/// Retrieves a user from the database by Telegram ID.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
///
/// # Returns
///
/// Returns `Ok(Some(User))` if the user is found, `Ok(None)` if not found,
/// or a database error.
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
            COALESCE(u.burn_subtitles, 0) as burn_subtitles,
            COALESCE(u.progress_bar_style, 'classic') as progress_bar_style,
            COALESCE(u.is_blocked, 0) as is_blocked,
            COALESCE(u.experimental_features, 0) as experimental_features
        FROM users u
        LEFT JOIN subscriptions s ON s.user_id = u.telegram_id
        WHERE u.telegram_id = ?",
    )?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        let telegram_id: i64 = row.get(0)?;
        let username: Option<String> = row.get(1)?;
        let plan: Plan = row.get(2)?;
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
        let progress_bar_style: String = row.get(14).unwrap_or_else(|_| "classic".to_string());
        let is_blocked: bool = row.get::<_, i32>(15).unwrap_or(0) != 0;
        let experimental_features: i32 = row.get(16).unwrap_or(0);

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
            progress_bar_style,
            is_blocked,
            experimental_features,
        }))
    } else {
        Ok(None)
    }
}

/// Updates the user's plan.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `plan` - New user plan (e.g. "free", "premium")
///
/// # Returns
///
/// Returns `Ok(())` on success or a database error.
pub fn update_user_plan(conn: &DbConnection, telegram_id: i64, plan: &str) -> Result<()> {
    if plan == "free" {
        conn.execute(
            "INSERT INTO subscriptions (user_id, plan, expires_at, telegram_charge_id, is_recurring)
             VALUES (?1, ?2, NULL, NULL, 0)
             ON CONFLICT(user_id) DO UPDATE SET
                plan = excluded.plan,
                expires_at = NULL,
                telegram_charge_id = NULL,
                is_recurring = 0,
                updated_at = CURRENT_TIMESTAMP",
            [&telegram_id as &dyn rusqlite::ToSql, &plan as &dyn rusqlite::ToSql],
        )?;
    } else {
        conn.execute(
            "INSERT INTO subscriptions (user_id, plan)
             VALUES (?1, ?2)
             ON CONFLICT(user_id) DO UPDATE SET
                plan = excluded.plan,
                updated_at = CURRENT_TIMESTAMP",
            [&telegram_id as &dyn rusqlite::ToSql, &plan as &dyn rusqlite::ToSql],
        )?;
    }
    conn.execute(
        "UPDATE users SET plan = ?1 WHERE telegram_id = ?2",
        [&plan as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Updates the user's plan and sets the subscription expiry date.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `plan` - New user plan (e.g. "free", "premium", "vip")
/// * `days` - Number of days the subscription is valid (None for unlimited/free)
///
/// # Returns
///
/// Returns `Ok(())` on success or a database error.
pub fn update_user_plan_with_expiry(
    conn: &DbConnection,
    telegram_id: i64,
    plan: &str,
    days: Option<i32>,
) -> Result<()> {
    if let Some(days_count) = days {
        // Set expiry date N days from now
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
        // For free plan or unlimited subscriptions, clear expiry date
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

/// Checks and updates expired subscriptions by downgrading them to free.
///
/// # Arguments
///
/// * `conn` - Database connection
///
/// # Returns
///
/// Returns the number of updated users.
pub fn expire_old_subscriptions(conn: &DbConnection) -> Result<usize> {
    let expired_user_ids = {
        let mut stmt = conn.prepare(
            "SELECT user_id FROM subscriptions
             WHERE expires_at IS NOT NULL
             AND expires_at < datetime('now', 'utc')
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
               AND expires_at < datetime('now', 'utc')
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

/// Logs a user request into the request history.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `user_id` - Telegram ID of the user
/// * `request_text` - Request text (usually a URL)
///
/// # Returns
///
/// Returns `Ok(())` on success or a database error.
pub fn log_request(conn: &DbConnection, user_id: i64, request_text: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO request_history (user_id, request_text) VALUES (?1, ?2)",
        [&user_id as &dyn rusqlite::ToSql, &request_text as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

// ==================== User Settings ====================

/// Gets the preferred download format of the user.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
///
/// # Returns
///
/// Returns the download format ("mp3", "mp4", "srt", "txt") or "mp3" by default
/// if the user is not found or an error occurred.
pub fn get_user_download_format(conn: &DbConnection, telegram_id: i64) -> Result<String> {
    let mut stmt = conn.prepare("SELECT download_format FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(row.get(0)?)
    } else {
        Ok("mp3".to_string())
    }
}

/// Sets the preferred download format of the user.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
/// * `format` - Download format: "mp3", "mp4", "srt", "txt"
///
/// # Returns
///
/// Returns `Ok(())` on success or a database error.
pub fn set_user_download_format(conn: &DbConnection, telegram_id: i64, format: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET download_format = ?1 WHERE telegram_id = ?2",
        [&format as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Gets the subtitle download setting of the user.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `telegram_id` - Telegram ID of the user
///
/// # Returns
///
/// Returns `true` if subtitle download is enabled, `false` if disabled or user not found.
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

/// Gets the subtitle burn-in setting for video.
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

/// Sets the subtitle burn-in setting for video.
pub fn set_user_burn_subtitles(conn: &DbConnection, telegram_id: i64, enabled: bool) -> Result<()> {
    let value = if enabled { 1 } else { 0 };
    conn.execute(
        "UPDATE users SET burn_subtitles = ?1 WHERE telegram_id = ?2",
        [&value as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Gets the experimental features setting for a user.
pub fn get_user_experimental_features(conn: &DbConnection, telegram_id: i64) -> Result<bool> {
    let mut stmt = conn.prepare("SELECT COALESCE(experimental_features, 0) FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        let experimental_features: i32 = row.get(0)?;
        Ok(experimental_features == 1)
    } else {
        Ok(false)
    }
}

/// Sets the experimental features setting for a user.
pub fn set_user_experimental_features(conn: &DbConnection, telegram_id: i64, enabled: bool) -> Result<()> {
    let value = if enabled { 1 } else { 0 };
    conn.execute(
        "UPDATE users SET experimental_features = ?1 WHERE telegram_id = ?2",
        [&value as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Gets the subtitle style settings for a user.
pub fn get_user_subtitle_style(conn: &DbConnection, telegram_id: i64) -> Result<SubtitleStyle> {
    let mut stmt = conn.prepare(
        "SELECT subtitle_font_size, subtitle_text_color, subtitle_outline_color, \
         subtitle_outline_width, subtitle_shadow, subtitle_position \
         FROM users WHERE telegram_id = ?",
    )?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(SubtitleStyle {
            font_size: row.get(0).unwrap_or_else(|_| "medium".to_string()),
            text_color: row.get(1).unwrap_or_else(|_| "white".to_string()),
            outline_color: row.get(2).unwrap_or_else(|_| "black".to_string()),
            outline_width: row.get(3).unwrap_or(2),
            shadow: row.get(4).unwrap_or(1),
            position: row.get(5).unwrap_or_else(|_| "bottom".to_string()),
            margin_v: 0,
            margin_h: 0,
            bold: 0,
        })
    } else {
        Ok(SubtitleStyle::default())
    }
}

pub fn set_user_subtitle_font_size(conn: &DbConnection, telegram_id: i64, value: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET subtitle_font_size = ?1 WHERE telegram_id = ?2",
        [&value as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

pub fn set_user_subtitle_text_color(conn: &DbConnection, telegram_id: i64, value: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET subtitle_text_color = ?1 WHERE telegram_id = ?2",
        [&value as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

pub fn set_user_subtitle_outline_color(conn: &DbConnection, telegram_id: i64, value: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET subtitle_outline_color = ?1 WHERE telegram_id = ?2",
        [&value as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

pub fn set_user_subtitle_outline_width(conn: &DbConnection, telegram_id: i64, value: i32) -> Result<()> {
    conn.execute(
        "UPDATE users SET subtitle_outline_width = ?1 WHERE telegram_id = ?2",
        [&value as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

pub fn set_user_subtitle_shadow(conn: &DbConnection, telegram_id: i64, value: i32) -> Result<()> {
    conn.execute(
        "UPDATE users SET subtitle_shadow = ?1 WHERE telegram_id = ?2",
        [&value as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

pub fn set_user_subtitle_position(conn: &DbConnection, telegram_id: i64, value: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET subtitle_position = ?1 WHERE telegram_id = ?2",
        [&value as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Gets the progress bar style of the user.
pub fn get_user_progress_bar_style(conn: &DbConnection, telegram_id: i64) -> Result<String> {
    let mut stmt = conn.prepare("SELECT progress_bar_style FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(row.get(0).unwrap_or_else(|_| "classic".to_string()))
    } else {
        Ok("classic".to_string())
    }
}

/// Sets the progress bar style of the user.
pub fn set_user_progress_bar_style(conn: &DbConnection, telegram_id: i64, style: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET progress_bar_style = ?1 WHERE telegram_id = ?2",
        [&style as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Gets the video quality setting of the user.
pub fn get_user_video_quality(conn: &DbConnection, telegram_id: i64) -> Result<String> {
    let mut stmt = conn.prepare("SELECT video_quality FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(row.get(0).unwrap_or_else(|_| "best".to_string()))
    } else {
        Ok("best".to_string())
    }
}

/// Sets the video quality setting of the user.
pub fn set_user_video_quality(conn: &DbConnection, telegram_id: i64, quality: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET video_quality = ?1 WHERE telegram_id = ?2",
        [&quality as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Gets the video send type for the user (0 = Media, 1 = Document).
pub fn get_user_send_as_document(conn: &DbConnection, telegram_id: i64) -> Result<i32> {
    let mut stmt = conn.prepare("SELECT send_as_document FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(row.get(0).unwrap_or(0))
    } else {
        Ok(0) // Default to Media
    }
}

/// Sets the video send type for the user.
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

/// Gets the audio send type for the user (0 = Media, 1 = Document).
pub fn get_user_send_audio_as_document(conn: &DbConnection, telegram_id: i64) -> Result<i32> {
    let mut stmt = conn.prepare("SELECT send_audio_as_document FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(row.get(0).unwrap_or(0))
    } else {
        Ok(0) // Default to Media
    }
}

/// Sets the audio send type for the user.
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

/// Gets the audio bitrate setting of the user.
pub fn get_user_audio_bitrate(conn: &DbConnection, telegram_id: i64) -> Result<String> {
    let mut stmt = conn.prepare("SELECT audio_bitrate FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(row.get(0).unwrap_or_else(|_| "320k".to_string()))
    } else {
        Ok("320k".to_string())
    }
}

/// Sets the audio bitrate setting of the user.
pub fn set_user_audio_bitrate(conn: &DbConnection, telegram_id: i64, bitrate: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET audio_bitrate = ?1 WHERE telegram_id = ?2",
        [&bitrate as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

/// Gets the preferred language of the user (IETF code, e.g. "en", "ru").
pub fn get_user_language(conn: &DbConnection, telegram_id: i64) -> Result<String> {
    let mut stmt = conn.prepare("SELECT language FROM users WHERE telegram_id = ?")?;
    let mut rows = stmt.query([&telegram_id as &dyn rusqlite::ToSql])?;

    if let Some(row) = rows.next()? {
        Ok(row.get(0).unwrap_or_else(|_| "ru".to_string()))
    } else {
        Ok("ru".to_string())
    }
}

/// Sets the preferred language of the user.
pub fn set_user_language(conn: &DbConnection, telegram_id: i64, language: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET language = ?1 WHERE telegram_id = ?2",
        [&language as &dyn rusqlite::ToSql, &telegram_id as &dyn rusqlite::ToSql],
    )?;
    Ok(())
}

// ==================== User Queries ====================

/// Returns total user counts grouped by plan and blocked status.
pub fn get_user_counts(conn: &DbConnection) -> Result<UserCounts> {
    let mut counts = UserCounts::default();
    let mut stmt = conn.prepare(
        "SELECT COALESCE(s.plan, u.plan) as effective_plan, COALESCE(u.is_blocked, 0) as is_blocked, COUNT(*)
         FROM users u
         LEFT JOIN subscriptions s ON s.user_id = u.telegram_id
         GROUP BY effective_plan, is_blocked",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i32>(1)?, row.get::<_, usize>(2)?))
    })?;
    for row in rows {
        let (plan, blocked, count) = row?;
        counts.total += count;
        if blocked != 0 {
            counts.blocked += count;
        }
        match plan.as_str() {
            "premium" => counts.premium += count,
            "vip" => counts.vip += count,
            _ => counts.free += count,
        }
    }
    Ok(counts)
}

/// Returns a paginated, filtered list of users.
///
/// `filter` values: `None` = all, `"free"`, `"premium"`, `"vip"`, `"blocked"`.
pub fn get_users_paginated(
    conn: &DbConnection,
    filter: Option<&str>,
    offset: usize,
    limit: usize,
) -> Result<(Vec<User>, usize)> {
    // Build WHERE clause
    let (where_clause, param): (&str, Option<&dyn rusqlite::ToSql>) = match filter {
        Some("free") => ("WHERE COALESCE(s.plan, u.plan) = 'free'", None),
        Some("premium") => ("WHERE COALESCE(s.plan, u.plan) = 'premium'", None),
        Some("vip") => ("WHERE COALESCE(s.plan, u.plan) = 'vip'", None),
        Some("blocked") => ("WHERE COALESCE(u.is_blocked, 0) = 1", None),
        _ => ("", None),
    };
    let _ = param; // unused, filters are literals

    // Count total matching
    let count_sql = format!(
        "SELECT COUNT(*) FROM users u LEFT JOIN subscriptions s ON s.user_id = u.telegram_id {}",
        where_clause
    );
    let total: usize = conn.query_row(&count_sql, [], |row| row.get(0))?;

    // Fetch page
    let query_sql = format!(
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
            COALESCE(u.burn_subtitles, 0) as burn_subtitles,
            COALESCE(u.progress_bar_style, 'classic') as progress_bar_style,
            COALESCE(u.is_blocked, 0) as is_blocked,
            COALESCE(u.experimental_features, 0) as experimental_features
        FROM users u
        LEFT JOIN subscriptions s ON s.user_id = u.telegram_id
        {}
        ORDER BY u.telegram_id
        LIMIT ?1 OFFSET ?2",
        where_clause
    );
    let mut stmt = conn.prepare(&query_sql)?;
    let rows = stmt.query_map(
        [&limit as &dyn rusqlite::ToSql, &offset as &dyn rusqlite::ToSql],
        |row| {
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
                progress_bar_style: row.get(14).unwrap_or_else(|_| "classic".to_string()),
                is_blocked: row.get::<_, i32>(15).unwrap_or(0) != 0,
                experimental_features: row.get(16).unwrap_or(0),
            })
        },
    )?;

    let mut users = Vec::new();
    for row in rows {
        users.push(row?);
    }
    Ok((users, total))
}

/// Checks if a user is blocked.
pub fn is_user_blocked(conn: &DbConnection, telegram_id: i64) -> Result<bool> {
    let blocked: i32 = conn
        .query_row(
            "SELECT COALESCE(is_blocked, 0) FROM users WHERE telegram_id = ?",
            [telegram_id],
            |row| row.get(0),
        )
        .unwrap_or(0);
    Ok(blocked != 0)
}

/// Sets the blocked status of a user.
pub fn set_user_blocked(conn: &DbConnection, telegram_id: i64, blocked: bool) -> Result<()> {
    conn.execute(
        "UPDATE users SET is_blocked = ?1 WHERE telegram_id = ?2",
        [
            &(blocked as i32) as &dyn rusqlite::ToSql,
            &telegram_id as &dyn rusqlite::ToSql,
        ],
    )?;
    Ok(())
}

/// Searches users by telegram_id or username (partial match).
pub fn search_users(conn: &DbConnection, query: &str) -> Result<Vec<User>> {
    let pattern = format!("%{}%", query);
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
            COALESCE(u.burn_subtitles, 0) as burn_subtitles,
            COALESCE(u.progress_bar_style, 'classic') as progress_bar_style,
            COALESCE(u.is_blocked, 0) as is_blocked,
            COALESCE(u.experimental_features, 0) as experimental_features
        FROM users u
        LEFT JOIN subscriptions s ON s.user_id = u.telegram_id
        WHERE CAST(u.telegram_id AS TEXT) LIKE ?1 OR COALESCE(u.username, '') LIKE ?1
        ORDER BY u.telegram_id
        LIMIT 20",
    )?;
    let rows = stmt.query_map([&pattern as &dyn rusqlite::ToSql], |row| {
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
            progress_bar_style: row.get(14).unwrap_or_else(|_| "classic".to_string()),
            is_blocked: row.get::<_, i32>(15).unwrap_or(0) != 0,
            experimental_features: row.get(16).unwrap_or(0),
        })
    })?;

    let mut users = Vec::new();
    for row in rows {
        users.push(row?);
    }
    Ok(users)
}

/// Gets the list of all users from the database.
///
/// # Arguments
///
/// * `conn` - Database connection
///
/// # Returns
///
/// Returns `Ok(Vec<User>)` with all users or a database error.
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
            COALESCE(u.burn_subtitles, 0) as burn_subtitles,
            COALESCE(u.progress_bar_style, 'classic') as progress_bar_style,
            COALESCE(u.is_blocked, 0) as is_blocked,
            COALESCE(u.experimental_features, 0) as experimental_features
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
            progress_bar_style: row.get(14).unwrap_or_else(|_| "classic".to_string()),
            is_blocked: row.get::<_, i32>(15).unwrap_or(0) != 0,
            experimental_features: row.get(16).unwrap_or(0),
        })
    })?;

    let mut users = Vec::new();
    for row in rows {
        users.push(row?);
    }
    Ok(users)
}
