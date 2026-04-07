use anyhow::{Context, Result};
use sqlx::Row;

use crate::download::audio_effects::{AudioEffectSession, MorphProfile};
use crate::storage::db::{self, AudioCutSession, CookiesUploadSession, PlayerSession, VideoClipSession};

use super::SharedStorage;

impl SharedStorage {
    pub async fn create_audio_effect_session(&self, session: &AudioEffectSession) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite create_audio_effect_session connection")?;
                db::create_audio_effect_session(&conn, session).context("sqlite create_audio_effect_session")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO audio_effect_sessions (
                        id, user_id, original_file_path, current_file_path, telegram_file_id,
                        original_message_id, title, duration, pitch_semitones, tempo_factor,
                        bass_gain_db, morph_profile, version, processing, created_at, expires_at
                     ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
                )
                .bind(&session.id)
                .bind(session.user_id)
                .bind(&session.original_file_path)
                .bind(&session.current_file_path)
                .bind(&session.telegram_file_id)
                .bind(session.original_message_id)
                .bind(&session.title)
                .bind(session.duration as i64)
                .bind(session.pitch_semitones as i16)
                .bind(session.tempo_factor as f64)
                .bind(session.bass_gain_db as i16)
                .bind(session.morph_profile.as_str())
                .bind(session.version as i64)
                .bind(if session.processing { 1_i32 } else { 0_i32 })
                .bind(session.created_at)
                .bind(session.expires_at)
                .execute(pg_pool)
                .await
                .context("postgres create_audio_effect_session")?;
                Ok(())
            }
        }
    }

    pub async fn get_audio_effect_session(&self, session_id: &str) -> Result<Option<AudioEffectSession>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_audio_effect_session connection")?;
                db::get_audio_effect_session(&conn, session_id).context("sqlite get_audio_effect_session")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT *
                     FROM audio_effect_sessions
                     WHERE id = $1",
                )
                .bind(session_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_audio_effect_session")?;
                row.map(map_pg_audio_effect_session).transpose()
            }
        }
    }

    pub async fn delete_expired_audio_sessions(&self) -> Result<Vec<AudioEffectSession>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite delete_expired_audio_sessions connection")?;
                db::delete_expired_audio_sessions(&conn).context("sqlite delete_expired_audio_sessions")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query(
                    "DELETE FROM audio_effect_sessions
                     WHERE expires_at < NOW()
                     RETURNING *",
                )
                .fetch_all(pg_pool)
                .await
                .context("postgres delete_expired_audio_sessions")?;
                rows.into_iter().map(map_pg_audio_effect_session).collect()
            }
        }
    }

    pub async fn update_audio_effect_session(
        &self,
        session_id: &str,
        pitch_semitones: i8,
        tempo_factor: f32,
        bass_gain_db: i8,
        morph_profile: &str,
        current_file_path: &str,
        version: u32,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite update_audio_effect_session connection")?;
                db::update_audio_effect_session(
                    &conn,
                    session_id,
                    pitch_semitones,
                    tempo_factor,
                    bass_gain_db,
                    morph_profile,
                    current_file_path,
                    version,
                )
                .context("sqlite update_audio_effect_session")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE audio_effect_sessions
                     SET pitch_semitones = $1,
                         tempo_factor = $2,
                         bass_gain_db = $3,
                         morph_profile = $4,
                         current_file_path = $5,
                         version = $6
                     WHERE id = $7",
                )
                .bind(pitch_semitones as i16)
                .bind(tempo_factor as f64)
                .bind(bass_gain_db as i16)
                .bind(morph_profile)
                .bind(current_file_path)
                .bind(version as i64)
                .bind(session_id)
                .execute(pg_pool)
                .await
                .context("postgres update_audio_effect_session")?;
                Ok(())
            }
        }
    }

    pub async fn set_audio_effect_session_processing(&self, session_id: &str, processing: bool) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite set_session_processing connection")?;
                db::set_session_processing(&conn, session_id, processing).context("sqlite set_session_processing")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE audio_effect_sessions
                     SET processing = $1
                     WHERE id = $2",
                )
                .bind(if processing { 1_i32 } else { 0_i32 })
                .bind(session_id)
                .execute(pg_pool)
                .await
                .context("postgres set_session_processing")?;
                Ok(())
            }
        }
    }

    pub async fn upsert_audio_cut_session(&self, session: &AudioCutSession) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite upsert_audio_cut_session connection")?;
                db::upsert_audio_cut_session(&conn, session).context("sqlite upsert_audio_cut_session")
            }
            Self::Postgres { pg_pool, .. } => {
                let mut tx = pg_pool
                    .begin()
                    .await
                    .context("postgres upsert_audio_cut_session begin")?;
                sqlx::query("DELETE FROM audio_cut_sessions WHERE user_id = $1")
                    .bind(session.user_id)
                    .execute(&mut *tx)
                    .await
                    .context("postgres upsert_audio_cut_session delete")?;
                sqlx::query(
                    "INSERT INTO audio_cut_sessions (id, user_id, audio_session_id, created_at, expires_at)
                     VALUES ($1, $2, $3, $4, $5)",
                )
                .bind(&session.id)
                .bind(session.user_id)
                .bind(&session.audio_session_id)
                .bind(session.created_at)
                .bind(session.expires_at)
                .execute(&mut *tx)
                .await
                .context("postgres upsert_audio_cut_session insert")?;
                tx.commit().await.context("postgres upsert_audio_cut_session commit")?;
                Ok(())
            }
        }
    }

    pub async fn get_active_audio_cut_session(&self, user_id: i64) -> Result<Option<AudioCutSession>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_active_audio_cut_session connection")?;
                db::get_active_audio_cut_session(&conn, user_id).context("sqlite get_active_audio_cut_session")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT *
                     FROM audio_cut_sessions
                     WHERE user_id = $1
                       AND expires_at > NOW()
                     ORDER BY created_at DESC
                     LIMIT 1",
                )
                .bind(user_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_active_audio_cut_session")?;
                row.map(map_pg_audio_cut_session).transpose()
            }
        }
    }

    pub async fn delete_audio_cut_session_by_user(&self, user_id: i64) -> Result<()> {
        self.delete_session_by_user(
            user_id,
            "audio_cut_sessions",
            "sqlite delete_audio_cut_session_by_user connection",
            db::delete_audio_cut_session_by_user,
        )
        .await
    }

    pub async fn upsert_video_clip_session(&self, session: &VideoClipSession) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite upsert_video_clip_session connection")?;
                db::upsert_video_clip_session(&conn, session).context("sqlite upsert_video_clip_session")
            }
            Self::Postgres { pg_pool, .. } => {
                let mut tx = pg_pool
                    .begin()
                    .await
                    .context("postgres upsert_video_clip_session begin")?;
                sqlx::query("DELETE FROM video_clip_sessions WHERE user_id = $1")
                    .bind(session.user_id)
                    .execute(&mut *tx)
                    .await
                    .context("postgres upsert_video_clip_session delete")?;
                sqlx::query(
                    "INSERT INTO video_clip_sessions (
                        id, user_id, source_download_id, source_kind, source_id, original_url,
                        output_kind, created_at, expires_at, subtitle_lang, custom_audio_file_id
                     ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
                )
                .bind(&session.id)
                .bind(session.user_id)
                .bind(session.source_download_id)
                .bind(session.source_kind.as_str())
                .bind(session.source_id)
                .bind(&session.original_url)
                .bind(session.output_kind.as_str())
                .bind(session.created_at)
                .bind(session.expires_at)
                .bind(&session.subtitle_lang)
                .bind(&session.custom_audio_file_id)
                .execute(&mut *tx)
                .await
                .context("postgres upsert_video_clip_session insert")?;
                tx.commit().await.context("postgres upsert_video_clip_session commit")?;
                Ok(())
            }
        }
    }

    pub async fn get_active_video_clip_session(&self, user_id: i64) -> Result<Option<VideoClipSession>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_active_video_clip_session connection")?;
                db::get_active_video_clip_session(&conn, user_id).context("sqlite get_active_video_clip_session")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT *
                     FROM video_clip_sessions
                     WHERE user_id = $1
                       AND expires_at > NOW()
                     ORDER BY created_at DESC
                     LIMIT 1",
                )
                .bind(user_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_active_video_clip_session")?;
                row.map(map_pg_video_clip_session).transpose()
            }
        }
    }

    pub async fn delete_video_clip_session_by_user(&self, user_id: i64) -> Result<()> {
        self.delete_session_by_user(
            user_id,
            "video_clip_sessions",
            "sqlite delete_video_clip_session_by_user connection",
            db::delete_video_clip_session_by_user,
        )
        .await
    }

    pub async fn upsert_cookies_upload_session(&self, session: &CookiesUploadSession) -> Result<()> {
        self.upsert_cookies_session(session, false).await
    }

    pub async fn get_active_cookies_upload_session(&self, user_id: i64) -> Result<Option<CookiesUploadSession>> {
        self.get_active_cookies_session(user_id, false).await
    }

    pub async fn delete_cookies_upload_session_by_user(&self, user_id: i64) -> Result<()> {
        self.delete_session_by_user(
            user_id,
            "cookies_upload_sessions",
            "sqlite delete_cookies_upload_session_by_user connection",
            db::delete_cookies_upload_session_by_user,
        )
        .await
    }

    pub async fn upsert_ig_cookies_upload_session(&self, session: &CookiesUploadSession) -> Result<()> {
        self.upsert_cookies_session(session, true).await
    }

    pub async fn get_active_ig_cookies_upload_session(&self, user_id: i64) -> Result<Option<CookiesUploadSession>> {
        self.get_active_cookies_session(user_id, true).await
    }

    pub async fn delete_ig_cookies_upload_session_by_user(&self, user_id: i64) -> Result<()> {
        self.delete_session_by_user(
            user_id,
            "ig_cookies_upload_sessions",
            "sqlite delete_ig_cookies_upload_session_by_user connection",
            db::delete_ig_cookies_upload_session_by_user,
        )
        .await
    }

    pub async fn create_lyrics_session(
        &self,
        id: &str,
        user_id: i64,
        artist: &str,
        title: &str,
        sections_json: &str,
        has_structure: bool,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite create_lyrics_session connection")?;
                db::create_lyrics_session(&conn, id, user_id, artist, title, sections_json, has_structure)
                    .context("sqlite create_lyrics_session")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO lyrics_sessions (
                        id, user_id, artist, title, sections_json, has_structure, created_at, expires_at
                     ) VALUES ($1, $2, $3, $4, $5, $6, NOW(), NOW() + INTERVAL '24 hours')",
                )
                .bind(id)
                .bind(user_id)
                .bind(artist)
                .bind(title)
                .bind(sections_json)
                .bind(if has_structure { 1_i32 } else { 0_i32 })
                .execute(pg_pool)
                .await
                .context("postgres create_lyrics_session")?;
                Ok(())
            }
        }
    }

    pub async fn get_lyrics_session(&self, id: &str) -> Result<Option<(String, String, String, bool)>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_lyrics_session connection")?;
                db::get_lyrics_session(&conn, id).context("sqlite get_lyrics_session")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT artist, title, sections_json, has_structure
                     FROM lyrics_sessions
                     WHERE id = $1
                       AND expires_at > NOW()",
                )
                .bind(id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_lyrics_session")?;
                Ok(row.map(|row| {
                    (
                        row.get("artist"),
                        row.get("title"),
                        row.get("sections_json"),
                        row.get::<i32, _>("has_structure") != 0,
                    )
                }))
            }
        }
    }

    pub async fn create_player_session(
        &self,
        user_id: i64,
        playlist_id: i64,
        player_message_id: Option<i32>,
        sticker_message_id: Option<i32>,
    ) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite create_player_session connection")?;
                db::create_player_session(&conn, user_id, playlist_id, player_message_id, sticker_message_id)
                    .context("sqlite create_player_session")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO player_sessions (
                        user_id, playlist_id, current_position, is_shuffle, repeat_mode, last_track_index,
                        player_message_id, sticker_message_id, updated_at
                     ) VALUES ($1, $2, 0, 0, 0, NULL, $3, $4, NOW())
                     ON CONFLICT (user_id) DO UPDATE SET
                        playlist_id = EXCLUDED.playlist_id,
                        current_position = 0,
                        is_shuffle = 0,
                        repeat_mode = 0,
                        last_track_index = NULL,
                        player_message_id = EXCLUDED.player_message_id,
                        sticker_message_id = EXCLUDED.sticker_message_id,
                        updated_at = NOW()",
                )
                .bind(user_id)
                .bind(playlist_id)
                .bind(player_message_id)
                .bind(sticker_message_id)
                .execute(pg_pool)
                .await
                .context("postgres create_player_session")?;
                Ok(())
            }
        }
    }

    pub async fn get_player_session(&self, user_id: i64) -> Result<Option<PlayerSession>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_player_session connection")?;
                db::get_player_session(&conn, user_id).context("sqlite get_player_session")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT user_id, playlist_id, current_position, is_shuffle,
                            COALESCE(repeat_mode, 0) AS repeat_mode,
                            last_track_index,
                            player_message_id, sticker_message_id,
                            updated_at::text AS updated_at
                     FROM player_sessions
                     WHERE user_id = $1",
                )
                .bind(user_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_player_session")?;
                Ok(row.map(|row| PlayerSession {
                    user_id: row.get("user_id"),
                    playlist_id: row.get("playlist_id"),
                    current_position: row.get("current_position"),
                    is_shuffle: row.get::<i32, _>("is_shuffle") != 0,
                    repeat_mode: row.get::<i32, _>("repeat_mode"),
                    last_track_index: row.get("last_track_index"),
                    player_message_id: row.get("player_message_id"),
                    sticker_message_id: row.get("sticker_message_id"),
                    updated_at: row.get("updated_at"),
                }))
            }
        }
    }

    pub async fn cycle_player_repeat(&self, user_id: i64) -> Result<i32> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite cycle_player_repeat connection")?;
                db::cycle_player_repeat(&conn, user_id).context("sqlite cycle_player_repeat")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "UPDATE player_sessions
                     SET repeat_mode = (COALESCE(repeat_mode, 0) + 1) % 3,
                         updated_at = NOW()
                     WHERE user_id = $1
                     RETURNING repeat_mode",
                )
                .bind(user_id)
                .fetch_one(pg_pool)
                .await
                .context("postgres cycle_player_repeat")?;
                Ok(row.get::<i32, _>("repeat_mode"))
            }
        }
    }

    pub async fn set_player_last_track_index(&self, user_id: i64, index: i32) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite set_player_last_track_index connection")?;
                db::set_player_last_track_index(&conn, user_id, index).context("sqlite set_player_last_track_index")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("UPDATE player_sessions SET last_track_index = $1, updated_at = NOW() WHERE user_id = $2")
                    .bind(index)
                    .bind(user_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres set_player_last_track_index")?;
                Ok(())
            }
        }
    }

    pub async fn clear_player_last_track_index(&self, user_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite clear_player_last_track_index connection")?;
                db::clear_player_last_track_index(&conn, user_id).context("sqlite clear_player_last_track_index")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "UPDATE player_sessions SET last_track_index = NULL, updated_at = NOW() WHERE user_id = $1",
                )
                .bind(user_id)
                .execute(pg_pool)
                .await
                .context("postgres clear_player_last_track_index")?;
                Ok(())
            }
        }
    }

    pub async fn toggle_player_shuffle(&self, user_id: i64) -> Result<bool> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite toggle_player_shuffle connection")?;
                db::toggle_player_shuffle(&conn, user_id).context("sqlite toggle_player_shuffle")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "UPDATE player_sessions
                     SET is_shuffle = CASE WHEN is_shuffle = 0 THEN 1 ELSE 0 END,
                         updated_at = NOW()
                     WHERE user_id = $1
                     RETURNING is_shuffle",
                )
                .bind(user_id)
                .fetch_one(pg_pool)
                .await
                .context("postgres toggle_player_shuffle")?;
                Ok(row.get::<i32, _>("is_shuffle") != 0)
            }
        }
    }

    pub async fn delete_player_session(&self, user_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite delete_player_session connection")?;
                db::delete_player_session(&conn, user_id).context("sqlite delete_player_session")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("DELETE FROM player_sessions WHERE user_id = $1")
                    .bind(user_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres delete_player_session")?;
                Ok(())
            }
        }
    }

    pub async fn add_player_message(&self, user_id: i64, message_id: i32) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite add_player_message connection")?;
                db::add_player_message(&conn, user_id, message_id).context("sqlite add_player_message")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO player_messages (user_id, message_id)
                     VALUES ($1, $2)
                     ON CONFLICT (user_id, message_id) DO NOTHING",
                )
                .bind(user_id)
                .bind(message_id)
                .execute(pg_pool)
                .await
                .context("postgres add_player_message")?;
                Ok(())
            }
        }
    }

    pub async fn get_player_messages(&self, user_id: i64) -> Result<Vec<i32>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_player_messages connection")?;
                db::get_player_messages(&conn, user_id).context("sqlite get_player_messages")
            }
            Self::Postgres { pg_pool, .. } => {
                let rows = sqlx::query("SELECT message_id FROM player_messages WHERE user_id = $1")
                    .bind(user_id)
                    .fetch_all(pg_pool)
                    .await
                    .context("postgres get_player_messages")?;
                Ok(rows.into_iter().map(|row| row.get("message_id")).collect())
            }
        }
    }

    pub async fn delete_player_messages(&self, user_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite delete_player_messages connection")?;
                db::delete_player_messages(&conn, user_id).context("sqlite delete_player_messages")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("DELETE FROM player_messages WHERE user_id = $1")
                    .bind(user_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres delete_player_messages")?;
                Ok(())
            }
        }
    }

    pub async fn create_new_category_session(&self, user_id: i64, download_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite create_new_category_session connection")?;
                db::create_new_category_session(&conn, user_id, download_id)
                    .context("sqlite create_new_category_session")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO new_category_sessions (user_id, download_id, created_at)
                     VALUES ($1, $2, NOW())
                     ON CONFLICT (user_id) DO UPDATE SET
                        download_id = EXCLUDED.download_id,
                        created_at = NOW()",
                )
                .bind(user_id)
                .bind(download_id)
                .execute(pg_pool)
                .await
                .context("postgres create_new_category_session")?;
                Ok(())
            }
        }
    }

    pub async fn get_active_new_category_session(&self, user_id: i64) -> Result<Option<i64>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_active_new_category_session connection")?;
                db::get_active_new_category_session(&conn, user_id).context("sqlite get_active_new_category_session")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT download_id
                     FROM new_category_sessions
                     WHERE user_id = $1
                       AND created_at > NOW() - INTERVAL '10 minutes'",
                )
                .bind(user_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_active_new_category_session")?;
                Ok(row.map(|row| row.get("download_id")))
            }
        }
    }

    pub async fn delete_new_category_session(&self, user_id: i64) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite delete_new_category_session connection")?;
                db::delete_new_category_session(&conn, user_id).context("sqlite delete_new_category_session")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query("DELETE FROM new_category_sessions WHERE user_id = $1")
                    .bind(user_id)
                    .execute(pg_pool)
                    .await
                    .context("postgres delete_new_category_session")?;
                Ok(())
            }
        }
    }

    // Private helpers for cookies sessions

    async fn upsert_cookies_session(&self, session: &CookiesUploadSession, instagram: bool) -> Result<()> {
        let table_name = if instagram {
            "ig_cookies_upload_sessions"
        } else {
            "cookies_upload_sessions"
        };
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite upsert_cookies_session connection")?;
                if instagram {
                    db::upsert_ig_cookies_upload_session(&conn, session)
                        .context("sqlite upsert_ig_cookies_upload_session")
                } else {
                    db::upsert_cookies_upload_session(&conn, session).context("sqlite upsert_cookies_upload_session")
                }
            }
            Self::Postgres { pg_pool, .. } => {
                let mut tx = pg_pool.begin().await.context("postgres upsert_cookies_session begin")?;
                sqlx::query(&format!("DELETE FROM {table_name} WHERE user_id = $1"))
                    .bind(session.user_id)
                    .execute(&mut *tx)
                    .await
                    .with_context(|| format!("postgres upsert_cookies_session delete from {table_name}"))?;
                sqlx::query(&format!(
                    "INSERT INTO {table_name} (id, user_id, created_at, expires_at) VALUES ($1, $2, $3, $4)"
                ))
                .bind(&session.id)
                .bind(session.user_id)
                .bind(session.created_at)
                .bind(session.expires_at)
                .execute(&mut *tx)
                .await
                .with_context(|| format!("postgres upsert_cookies_session insert into {table_name}"))?;
                tx.commit().await.context("postgres upsert_cookies_session commit")?;
                Ok(())
            }
        }
    }

    async fn get_active_cookies_session(&self, user_id: i64, instagram: bool) -> Result<Option<CookiesUploadSession>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_active_cookies_session connection")?;
                if instagram {
                    db::get_active_ig_cookies_upload_session(&conn, user_id)
                        .context("sqlite get_active_ig_cookies_upload_session")
                } else {
                    db::get_active_cookies_upload_session(&conn, user_id)
                        .context("sqlite get_active_cookies_upload_session")
                }
            }
            Self::Postgres { pg_pool, .. } => {
                let table_name = if instagram {
                    "ig_cookies_upload_sessions"
                } else {
                    "cookies_upload_sessions"
                };
                let row = sqlx::query(&format!(
                    "SELECT * FROM {table_name} WHERE user_id = $1 AND expires_at > NOW() ORDER BY created_at DESC LIMIT 1"
                ))
                .bind(user_id)
                .fetch_optional(pg_pool)
                .await
                .with_context(|| format!("postgres get_active_cookies_session from {table_name}"))?;
                row.map(map_pg_cookies_upload_session).transpose()
            }
        }
    }
}

fn map_pg_audio_effect_session(row: sqlx::postgres::PgRow) -> Result<AudioEffectSession> {
    Ok(AudioEffectSession {
        id: row.get("id"),
        user_id: row.get("user_id"),
        original_file_path: row.get("original_file_path"),
        current_file_path: row.get("current_file_path"),
        telegram_file_id: row.get("telegram_file_id"),
        original_message_id: row.get("original_message_id"),
        title: row.get("title"),
        duration: row.get::<i64, _>("duration") as u32,
        pitch_semitones: row.get::<i16, _>("pitch_semitones") as i8,
        tempo_factor: row.get::<f64, _>("tempo_factor") as f32,
        bass_gain_db: row.get::<i16, _>("bass_gain_db") as i8,
        morph_profile: MorphProfile::parse(row.get::<String, _>("morph_profile").as_str()),
        version: row.get::<i64, _>("version") as u32,
        processing: row.get::<i32, _>("processing") != 0,
        created_at: row.get("created_at"),
        expires_at: row.get("expires_at"),
    })
}

fn map_pg_audio_cut_session(row: sqlx::postgres::PgRow) -> Result<AudioCutSession> {
    Ok(AudioCutSession {
        id: row.get("id"),
        user_id: row.get("user_id"),
        audio_session_id: row.get("audio_session_id"),
        created_at: row.get("created_at"),
        expires_at: row.get("expires_at"),
    })
}

fn map_pg_video_clip_session(row: sqlx::postgres::PgRow) -> Result<VideoClipSession> {
    use crate::storage::db::{OutputKind, SourceKind};
    Ok(VideoClipSession {
        id: row.get("id"),
        user_id: row.get("user_id"),
        source_download_id: row.get("source_download_id"),
        source_kind: SourceKind::from_str_lossy(&row.get::<String, _>("source_kind")),
        source_id: row.get("source_id"),
        original_url: row.get("original_url"),
        output_kind: OutputKind::from_str_lossy(&row.get::<String, _>("output_kind")),
        created_at: row.get("created_at"),
        expires_at: row.get("expires_at"),
        subtitle_lang: row.get("subtitle_lang"),
        custom_audio_file_id: row.get("custom_audio_file_id"),
    })
}

fn map_pg_cookies_upload_session(row: sqlx::postgres::PgRow) -> Result<CookiesUploadSession> {
    Ok(CookiesUploadSession {
        id: row.get("id"),
        user_id: row.get("user_id"),
        created_at: row.get("created_at"),
        expires_at: row.get("expires_at"),
    })
}
