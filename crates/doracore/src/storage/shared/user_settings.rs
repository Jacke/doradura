use anyhow::{Context, Result};
use sqlx::Row;

use crate::storage::db::{self, SubtitleStyle};

use super::SharedStorage;

impl SharedStorage {
    pub async fn get_user_language(&self, telegram_id: i64) -> Result<String> {
        self.get_user_string_setting(
            telegram_id,
            "language",
            "SELECT language FROM users WHERE telegram_id = $1",
            "ru",
        )
        .await
    }

    pub async fn get_user_progress_bar_style(&self, telegram_id: i64) -> Result<String> {
        self.get_user_string_setting(
            telegram_id,
            "progress_bar_style",
            "SELECT progress_bar_style FROM users WHERE telegram_id = $1",
            "classic",
        )
        .await
    }

    pub async fn get_user_video_quality(&self, telegram_id: i64) -> Result<String> {
        self.get_user_string_setting(
            telegram_id,
            "video_quality",
            "SELECT video_quality FROM users WHERE telegram_id = $1",
            "best",
        )
        .await
    }

    pub async fn get_user_video_quality_preset(&self, telegram_id: i64) -> Result<String> {
        self.get_user_string_setting(
            telegram_id,
            "video_quality_preset",
            "SELECT COALESCE(video_quality_preset, 'master') AS video_quality_preset FROM users WHERE telegram_id = $1",
            "master",
        )
        .await
    }

    pub async fn get_user_download_format(&self, telegram_id: i64) -> Result<String> {
        self.get_user_string_setting(
            telegram_id,
            "download_format",
            "SELECT download_format FROM users WHERE telegram_id = $1",
            "mp3",
        )
        .await
    }

    pub async fn get_user_audio_bitrate(&self, telegram_id: i64) -> Result<String> {
        self.get_user_string_setting(
            telegram_id,
            "audio_bitrate",
            "SELECT audio_bitrate FROM users WHERE telegram_id = $1",
            "320k",
        )
        .await
    }

    pub async fn get_user_send_as_document(&self, telegram_id: i64) -> Result<i32> {
        self.get_user_i32_setting(
            telegram_id,
            "send_as_document",
            "SELECT send_as_document FROM users WHERE telegram_id = $1",
            0,
        )
        .await
    }

    pub async fn get_user_send_audio_as_document(&self, telegram_id: i64) -> Result<i32> {
        self.get_user_i32_setting(
            telegram_id,
            "send_audio_as_document",
            "SELECT send_audio_as_document FROM users WHERE telegram_id = $1",
            0,
        )
        .await
    }

    pub async fn get_user_download_subtitles(&self, telegram_id: i64) -> Result<bool> {
        Ok(self
            .get_user_i32_setting(
                telegram_id,
                "download_subtitles",
                "SELECT download_subtitles FROM users WHERE telegram_id = $1",
                0,
            )
            .await?
            == 1)
    }

    pub async fn get_user_burn_subtitles(&self, telegram_id: i64) -> Result<bool> {
        Ok(self
            .get_user_i32_setting(
                telegram_id,
                "burn_subtitles",
                "SELECT COALESCE(burn_subtitles, 0) AS burn_subtitles FROM users WHERE telegram_id = $1",
                0,
            )
            .await?
            == 1)
    }

    pub async fn get_user_video_no_caption(&self, telegram_id: i64) -> Result<bool> {
        Ok(self
            .get_user_i32_setting(
                telegram_id,
                "video_no_caption",
                "SELECT COALESCE(video_no_caption, 0) AS video_no_caption FROM users WHERE telegram_id = $1",
                0,
            )
            .await?
            == 1)
    }

    pub async fn get_user_experimental_features(&self, telegram_id: i64) -> Result<bool> {
        Ok(self
            .get_user_i32_setting(
                telegram_id,
                "experimental_features",
                "SELECT COALESCE(experimental_features, 0) AS experimental_features FROM users WHERE telegram_id = $1",
                0,
            )
            .await?
            == 1)
    }

    pub async fn get_user_subtitle_style(&self, telegram_id: i64) -> Result<SubtitleStyle> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_user_subtitle_style connection")?;
                db::get_user_subtitle_style(&conn, telegram_id).context("sqlite get_user_subtitle_style")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT
                        COALESCE(subtitle_font_size, 'medium') AS subtitle_font_size,
                        COALESCE(subtitle_text_color, 'white') AS subtitle_text_color,
                        COALESCE(subtitle_outline_color, 'black') AS subtitle_outline_color,
                        COALESCE(subtitle_outline_width, 2) AS subtitle_outline_width,
                        COALESCE(subtitle_shadow, 1) AS subtitle_shadow,
                        COALESCE(subtitle_position, 'bottom') AS subtitle_position
                     FROM users
                     WHERE telegram_id = $1",
                )
                .bind(telegram_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_user_subtitle_style")?;
                Ok(row.map(map_pg_subtitle_style).unwrap_or_default())
            }
        }
    }

    pub async fn set_user_video_quality(&self, telegram_id: i64, quality: &str) -> Result<()> {
        self.set_user_string_setting(
            telegram_id,
            "video_quality",
            quality,
            "UPDATE users SET video_quality = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_video_quality_preset(&self, telegram_id: i64, preset: &str) -> Result<()> {
        self.set_user_string_setting(
            telegram_id,
            "video_quality_preset",
            preset,
            "UPDATE users SET video_quality_preset = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_download_format(&self, telegram_id: i64, format: &str) -> Result<()> {
        self.set_user_string_setting(
            telegram_id,
            "download_format",
            format,
            "UPDATE users SET download_format = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_audio_bitrate(&self, telegram_id: i64, bitrate: &str) -> Result<()> {
        self.set_user_string_setting(
            telegram_id,
            "audio_bitrate",
            bitrate,
            "UPDATE users SET audio_bitrate = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_send_as_document(&self, telegram_id: i64, send_as_document: i32) -> Result<()> {
        self.set_user_i32_setting(
            telegram_id,
            "send_as_document",
            send_as_document,
            "UPDATE users SET send_as_document = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_send_audio_as_document(&self, telegram_id: i64, send_audio_as_document: i32) -> Result<()> {
        self.set_user_i32_setting(
            telegram_id,
            "send_audio_as_document",
            send_audio_as_document,
            "UPDATE users SET send_audio_as_document = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_burn_subtitles(&self, telegram_id: i64, enabled: bool) -> Result<()> {
        self.set_user_i32_setting(
            telegram_id,
            "burn_subtitles",
            i32::from(enabled),
            "UPDATE users SET burn_subtitles = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_video_no_caption(&self, telegram_id: i64, enabled: bool) -> Result<()> {
        self.set_user_i32_setting(
            telegram_id,
            "video_no_caption",
            i32::from(enabled),
            "UPDATE users SET video_no_caption = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_experimental_features(&self, telegram_id: i64, enabled: bool) -> Result<()> {
        self.set_user_i32_setting(
            telegram_id,
            "experimental_features",
            i32::from(enabled),
            "UPDATE users SET experimental_features = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_language(&self, telegram_id: i64, language: &str) -> Result<()> {
        self.set_user_string_setting(
            telegram_id,
            "language",
            language,
            "UPDATE users SET language = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_subtitle_font_size(&self, telegram_id: i64, value: &str) -> Result<()> {
        self.set_user_string_setting(
            telegram_id,
            "subtitle_font_size",
            value,
            "UPDATE users SET subtitle_font_size = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_subtitle_text_color(&self, telegram_id: i64, value: &str) -> Result<()> {
        self.set_user_string_setting(
            telegram_id,
            "subtitle_text_color",
            value,
            "UPDATE users SET subtitle_text_color = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_subtitle_outline_color(&self, telegram_id: i64, value: &str) -> Result<()> {
        self.set_user_string_setting(
            telegram_id,
            "subtitle_outline_color",
            value,
            "UPDATE users SET subtitle_outline_color = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_subtitle_outline_width(&self, telegram_id: i64, value: i32) -> Result<()> {
        self.set_user_i32_setting(
            telegram_id,
            "subtitle_outline_width",
            value,
            "UPDATE users SET subtitle_outline_width = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_subtitle_shadow(&self, telegram_id: i64, value: i32) -> Result<()> {
        self.set_user_i32_setting(
            telegram_id,
            "subtitle_shadow",
            value,
            "UPDATE users SET subtitle_shadow = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_subtitle_position(&self, telegram_id: i64, value: &str) -> Result<()> {
        self.set_user_string_setting(
            telegram_id,
            "subtitle_position",
            value,
            "UPDATE users SET subtitle_position = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn set_user_progress_bar_style(&self, telegram_id: i64, style: &str) -> Result<()> {
        self.set_user_string_setting(
            telegram_id,
            "progress_bar_style",
            style,
            "UPDATE users SET progress_bar_style = $2, updated_at = NOW() WHERE telegram_id = $1",
        )
        .await
    }

    pub async fn get_bot_asset(&self, key: &str) -> Result<Option<String>> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite get_bot_asset connection")?;
                db::get_bot_asset(&conn, key).context("sqlite get_bot_asset")
            }
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query("SELECT file_id FROM bot_assets WHERE key = $1")
                    .bind(key)
                    .fetch_optional(pg_pool)
                    .await
                    .context("postgres get_bot_asset")?;
                Ok(row.map(|row| row.get("file_id")))
            }
        }
    }

    pub async fn set_bot_asset(&self, key: &str, file_id: &str) -> Result<()> {
        match self {
            Self::Sqlite { db_pool } => {
                let conn = db::get_connection(db_pool).context("sqlite set_bot_asset connection")?;
                db::set_bot_asset(&conn, key, file_id).context("sqlite set_bot_asset")
            }
            Self::Postgres { pg_pool, .. } => {
                sqlx::query(
                    "INSERT INTO bot_assets (key, file_id, created_at)
                     VALUES ($1, $2, NOW())
                     ON CONFLICT (key) DO UPDATE SET file_id = EXCLUDED.file_id, created_at = NOW()",
                )
                .bind(key)
                .bind(file_id)
                .execute(pg_pool)
                .await
                .context("postgres set_bot_asset")?;
                Ok(())
            }
        }
    }

    /// Single-query bundle of user settings used during a video download.
    ///
    /// Replaces 4 round-trips (`get_user_video_quality_preset`,
    /// `get_user_experimental_features`, `get_user_send_as_document`,
    /// `get_user_video_no_caption`) with one Postgres SELECT. SQLite path
    /// falls back to the per-field calls — local dev only, perf not critical.
    pub async fn get_user_video_download_settings(&self, telegram_id: i64) -> Result<VideoDownloadSettings> {
        match self {
            Self::Sqlite { .. } => Ok(VideoDownloadSettings {
                quality_preset: self
                    .get_user_video_quality_preset(telegram_id)
                    .await
                    .unwrap_or_else(|_| "master".to_string()),
                experimental_features: self.get_user_experimental_features(telegram_id).await.unwrap_or(false),
                send_as_document: self.get_user_send_as_document(telegram_id).await.unwrap_or(0) != 0,
                video_no_caption: self.get_user_video_no_caption(telegram_id).await.unwrap_or(false),
            }),
            Self::Postgres { pg_pool, .. } => {
                let row = sqlx::query(
                    "SELECT
                        COALESCE(video_quality_preset, 'master')      AS video_quality_preset,
                        COALESCE(experimental_features, 0)            AS experimental_features,
                        COALESCE(send_as_document, 0)                 AS send_as_document,
                        COALESCE(video_no_caption, 0)                 AS video_no_caption
                     FROM users
                     WHERE telegram_id = $1",
                )
                .bind(telegram_id)
                .fetch_optional(pg_pool)
                .await
                .context("postgres get_user_video_download_settings")?;
                Ok(row
                    .map(|row| VideoDownloadSettings {
                        quality_preset: row.get::<String, _>("video_quality_preset"),
                        experimental_features: row.get::<i32, _>("experimental_features") != 0,
                        send_as_document: row.get::<i32, _>("send_as_document") != 0,
                        video_no_caption: row.get::<i32, _>("video_no_caption") != 0,
                    })
                    .unwrap_or_default())
            }
        }
    }
}

/// Bundled user settings consumed by the video download pipeline.
/// Fetched once per request to collapse 4 N+1 SELECTs into 1.
#[derive(Debug, Clone)]
pub struct VideoDownloadSettings {
    pub quality_preset: String,
    pub experimental_features: bool,
    pub send_as_document: bool,
    pub video_no_caption: bool,
}

impl Default for VideoDownloadSettings {
    fn default() -> Self {
        Self {
            quality_preset: "master".to_string(),
            experimental_features: false,
            send_as_document: false,
            video_no_caption: false,
        }
    }
}

fn map_pg_subtitle_style(row: sqlx::postgres::PgRow) -> SubtitleStyle {
    SubtitleStyle {
        font_size: row.get("subtitle_font_size"),
        text_color: row.get("subtitle_text_color"),
        outline_color: row.get("subtitle_outline_color"),
        outline_width: row.get("subtitle_outline_width"),
        shadow: row.get("subtitle_shadow"),
        position: row.get("subtitle_position"),
        margin_v: row.try_get("subtitle_margin_v").unwrap_or(0),
        margin_h: row.try_get("subtitle_margin_h").unwrap_or(0),
        bold: row.try_get("subtitle_bold").unwrap_or(0),
    }
}
