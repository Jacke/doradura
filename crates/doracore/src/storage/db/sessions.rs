//! Session management: audio effects, audio cut, video clip, and cookies upload sessions.
//!
//! Extracted from the main `db` module for better organization.

use super::{get_user, DbConnection};
use rusqlite::Result;
use std::sync::Once;

// ==================== Audio Effect Sessions ====================

/// Check if user has an ACTIVE paid subscription (plan + unexpired expires_at).
///
/// Uses `User::is_subscription_active()` which verifies expiry, closing the
/// ~1-hour window between real expiry and the hourly reaper.
pub fn is_premium_or_vip(conn: &DbConnection, user_id: i64) -> Result<bool> {
    let user = get_user(conn, user_id)?;
    Ok(user.map(|u| u.is_subscription_active()).unwrap_or(false))
}

static BASS_COLUMN_INIT: Once = Once::new();
static MORPH_COLUMN_INIT: Once = Once::new();

fn ensure_audio_effects_bass_column(conn: &DbConnection) {
    BASS_COLUMN_INIT.call_once(|| {
        let _ = conn.execute(
            "ALTER TABLE audio_effect_sessions ADD COLUMN bass_gain_db INTEGER DEFAULT 0",
            [],
        );
    });
}

fn ensure_audio_effects_morph_column(conn: &DbConnection) {
    MORPH_COLUMN_INIT.call_once(|| {
        let _ = conn.execute(
            "ALTER TABLE audio_effect_sessions ADD COLUMN morph_profile TEXT DEFAULT 'none'",
            [],
        );
    });
}

fn ensure_audio_effects_columns(conn: &DbConnection) {
    ensure_audio_effects_bass_column(conn);
    ensure_audio_effects_morph_column(conn);
}

/// Parse an RFC3339 datetime string, falling back to `fallback` on error.
fn parse_dt(s: &str, fallback: chrono::DateTime<chrono::Utc>) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or(fallback)
}

/// Map a row from the audio_effect_sessions table to an `AudioEffectSession`.
///
/// Expects columns in SELECT order: id, user_id, original_file_path,
/// current_file_path, telegram_file_id, original_message_id, title,
/// duration, pitch_semitones, tempo_factor, bass_gain_db, morph_profile,
/// version, processing, created_at, expires_at
fn audio_effect_session_from_row(
    row: &rusqlite::Row,
) -> rusqlite::Result<crate::download::audio_effects::AudioEffectSession> {
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
        created_at: parse_dt(&row.get::<_, String>(14)?, chrono::Utc::now()),
        expires_at: parse_dt(
            &row.get::<_, String>(15)?,
            chrono::Utc::now() + chrono::Duration::hours(24),
        ),
    })
}

/// The SELECT column list for audio_effect_sessions (shared across queries).
const AUDIO_SESSION_COLS: &str = "id, user_id, original_file_path, current_file_path, telegram_file_id, \
     original_message_id, title, duration, pitch_semitones, tempo_factor, bass_gain_db, morph_profile, \
     version, processing, created_at, expires_at";

/// Create a new audio effect session
pub fn create_audio_effect_session(
    conn: &DbConnection,
    session: &crate::download::audio_effects::AudioEffectSession,
) -> Result<()> {
    ensure_audio_effects_columns(conn);

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
    ensure_audio_effects_columns(conn);
    let sql = format!("SELECT {} FROM audio_effect_sessions WHERE id = ?1", AUDIO_SESSION_COLS);
    let mut stmt = conn.prepare(&sql)?;

    let result = stmt.query_row([session_id], audio_effect_session_from_row);

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
    ensure_audio_effects_columns(conn);
    let sql = format!(
        "SELECT {} FROM audio_effect_sessions WHERE user_id = ?1 AND original_message_id = ?2",
        AUDIO_SESSION_COLS
    );
    let mut stmt = conn.prepare(&sql)?;

    let result = stmt.query_row([user_id, message_id as i64], audio_effect_session_from_row);

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
    ensure_audio_effects_columns(conn);
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

/// Update download history with message_id and chat_id for MTProto refresh
///
/// This allows fetching fresh file_reference via messages.getMessages
pub fn update_download_message_id(conn: &DbConnection, download_id: i64, message_id: i32, chat_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE download_history SET message_id = ?1, chat_id = ?2 WHERE id = ?3",
        rusqlite::params![message_id, chat_id, download_id],
    )?;
    Ok(())
}

/// Update cut entry with message_id and chat_id for MTProto refresh
pub fn update_cut_message_id(conn: &DbConnection, cut_id: i64, message_id: i32, chat_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE cuts SET message_id = ?1, chat_id = ?2 WHERE id = ?3",
        rusqlite::params![message_id, chat_id, cut_id],
    )?;
    Ok(())
}

/// Get message_id and chat_id for a given table entry (shared helper).
fn get_message_info(conn: &DbConnection, table: &str, entry_id: i64) -> Result<Option<(i32, i64)>> {
    let sql = format!("SELECT message_id, chat_id FROM {} WHERE id = ?1", table);
    let mut stmt = conn.prepare(&sql)?;
    let result = stmt.query_row([entry_id], |row| {
        let msg_id: Option<i32> = row.get(0)?;
        let chat_id: Option<i64> = row.get(1)?;
        Ok((msg_id, chat_id))
    });

    match result {
        Ok((Some(msg_id), Some(chat_id))) => Ok(Some((msg_id, chat_id))),
        Ok(_) => Ok(None),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Get message_id and chat_id for a download entry (for MTProto fallback)
pub fn get_download_message_info(conn: &DbConnection, download_id: i64) -> Result<Option<(i32, i64)>> {
    get_message_info(conn, "download_history", download_id)
}

/// Get message_id and chat_id for a cut entry (for MTProto fallback)
pub fn get_cut_message_info(conn: &DbConnection, cut_id: i64) -> Result<Option<(i32, i64)>> {
    get_message_info(conn, "cuts", cut_id)
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
    ensure_audio_effects_columns(conn);
    let now = chrono::Utc::now().to_rfc3339();

    // Get expired sessions
    let sql = format!(
        "SELECT {} FROM audio_effect_sessions WHERE expires_at < ?1",
        AUDIO_SESSION_COLS
    );
    let mut stmt = conn.prepare(&sql)?;
    let sessions: Vec<crate::download::audio_effects::AudioEffectSession> = stmt
        .query_map([&now], audio_effect_session_from_row)?
        .collect::<Result<Vec<_>>>()?;

    // Batch delete all expired sessions in one statement
    conn.execute("DELETE FROM audio_effect_sessions WHERE expires_at < ?1", [&now])?;

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
        Ok(Some(AudioCutSession {
            id: row.get(0)?,
            user_id: row.get(1)?,
            audio_session_id: row.get(2)?,
            created_at: parse_dt(&row.get::<_, String>(3)?, chrono::Utc::now()),
            expires_at: parse_dt(
                &row.get::<_, String>(4)?,
                chrono::Utc::now() + chrono::Duration::minutes(10),
            ),
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

/// Kind of output produced by a video clip session.
///
/// `Display` / `as_str` are derived via strum with `serialize_all = "snake_case"`.
/// `from_str_lossy` stays manual because it has a "fall back to `Cut` on
/// unknown input" contract that strum's `EnumString` doesn't express.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display, strum::AsRefStr, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum OutputKind {
    Cut,
    VideoNote,
    Gif,
    IphoneRingtone,
    AndroidRingtone,
}

impl OutputKind {
    /// Alias for `Into::<&'static str>::into` — keeps existing call sites
    /// unchanged.
    pub fn as_str(&self) -> &'static str {
        self.into()
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "video_note" => Self::VideoNote,
            "gif" => Self::Gif,
            "iphone_ringtone" => Self::IphoneRingtone,
            "android_ringtone" => Self::AndroidRingtone,
            _ => Self::Cut,
        }
    }
}

/// Source of a video clip (original download or a previous cut).
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display, strum::AsRefStr, strum::IntoStaticStr)]
#[strum(serialize_all = "lowercase")]
pub enum SourceKind {
    Download,
    Cut,
}

impl SourceKind {
    /// Alias for `Into::<&'static str>::into` — keeps existing call sites
    /// unchanged.
    pub fn as_str(&self) -> &'static str {
        self.into()
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "cut" => Self::Cut,
            _ => Self::Download,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VideoClipSession {
    pub id: String,
    pub user_id: i64,
    pub source_download_id: i64,
    pub source_kind: SourceKind,
    pub source_id: i64,
    pub original_url: String,
    pub output_kind: OutputKind,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub subtitle_lang: Option<String>,
    pub custom_audio_file_id: Option<String>,
}

pub fn upsert_video_clip_session(conn: &DbConnection, session: &VideoClipSession) -> Result<()> {
    conn.execute("DELETE FROM video_clip_sessions WHERE user_id = ?1", [session.user_id])?;
    conn.execute(
        "INSERT INTO video_clip_sessions (
            id, user_id, source_download_id, source_kind, source_id, original_url, output_kind, created_at, expires_at, subtitle_lang, custom_audio_file_id
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params![
            session.id,
            session.user_id,
            session.source_download_id,
            session.source_kind.as_str(),
            session.source_id,
            session.original_url,
            session.output_kind.as_str(),
            session.created_at.to_rfc3339(),
            session.expires_at.to_rfc3339(),
            session.subtitle_lang,
            session.custom_audio_file_id,
        ],
    )?;
    Ok(())
}

pub fn get_active_video_clip_session(conn: &DbConnection, user_id: i64) -> Result<Option<VideoClipSession>> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut stmt = conn.prepare(
        "SELECT id, user_id, source_download_id, source_kind, source_id, original_url, output_kind, created_at, expires_at, subtitle_lang, custom_audio_file_id
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
        let resolved_source_kind = SourceKind::from_str_lossy(&source_kind.unwrap_or_default());
        let resolved_source_id = source_id.unwrap_or(source_download_id);
        let resolved_original_url = original_url.unwrap_or_default();
        let resolved_output_kind = OutputKind::from_str_lossy(&output_kind.unwrap_or_default());
        Ok(Some(VideoClipSession {
            id: row.get(0)?,
            user_id: row.get(1)?,
            source_download_id,
            source_kind: resolved_source_kind,
            source_id: resolved_source_id,
            original_url: resolved_original_url,
            output_kind: resolved_output_kind,
            created_at: parse_dt(&row.get::<_, String>(7)?, chrono::Utc::now()),
            expires_at: parse_dt(
                &row.get::<_, String>(8)?,
                chrono::Utc::now() + chrono::Duration::minutes(10),
            ),
            subtitle_lang: row.get(9)?,
            custom_audio_file_id: row.get(10)?,
        }))
    } else {
        Ok(None)
    }
}

pub fn delete_video_clip_session_by_user(conn: &DbConnection, user_id: i64) -> Result<()> {
    conn.execute("DELETE FROM video_clip_sessions WHERE user_id = ?1", [user_id])?;
    Ok(())
}

// ==================== Bot Assets ====================
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
        Ok(Some(CookiesUploadSession {
            id: row.get(0)?,
            user_id: row.get(1)?,
            created_at: parse_dt(&row.get::<_, String>(2)?, chrono::Utc::now()),
            expires_at: parse_dt(
                &row.get::<_, String>(3)?,
                chrono::Utc::now() + chrono::Duration::minutes(10),
            ),
        }))
    } else {
        Ok(None)
    }
}

pub fn delete_cookies_upload_session_by_user(conn: &DbConnection, user_id: i64) -> Result<()> {
    conn.execute("DELETE FROM cookies_upload_sessions WHERE user_id = ?1", [user_id])?;
    Ok(())
}

// ==================== Instagram Cookies Upload Sessions ====================

pub fn upsert_ig_cookies_upload_session(conn: &DbConnection, session: &CookiesUploadSession) -> Result<()> {
    conn.execute(
        "DELETE FROM ig_cookies_upload_sessions WHERE user_id = ?1",
        [session.user_id],
    )?;
    conn.execute(
        "INSERT INTO ig_cookies_upload_sessions (id, user_id, created_at, expires_at)
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

pub fn get_active_ig_cookies_upload_session(conn: &DbConnection, user_id: i64) -> Result<Option<CookiesUploadSession>> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut stmt = conn.prepare(
        "SELECT id, user_id, created_at, expires_at
         FROM ig_cookies_upload_sessions
         WHERE user_id = ?1 AND expires_at > ?2
         ORDER BY created_at DESC
         LIMIT 1",
    )?;
    let mut rows = stmt.query(rusqlite::params![user_id, now])?;

    if let Some(row) = rows.next()? {
        Ok(Some(CookiesUploadSession {
            id: row.get(0)?,
            user_id: row.get(1)?,
            created_at: parse_dt(&row.get::<_, String>(2)?, chrono::Utc::now()),
            expires_at: parse_dt(
                &row.get::<_, String>(3)?,
                chrono::Utc::now() + chrono::Duration::minutes(10),
            ),
        }))
    } else {
        Ok(None)
    }
}

pub fn delete_ig_cookies_upload_session_by_user(conn: &DbConnection, user_id: i64) -> Result<()> {
    conn.execute("DELETE FROM ig_cookies_upload_sessions WHERE user_id = ?1", [user_id])?;
    Ok(())
}
