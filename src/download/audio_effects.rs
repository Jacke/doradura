use crate::core::error::AppError;
use crate::storage::db::DbPool;
use chrono::{DateTime, Duration, Utc};
use std::process::Command;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// Maximum concurrent audio effect processing tasks
const MAX_CONCURRENT_PROCESSING: usize = 3;

/// Semaphore to limit concurrent FFmpeg processing
static PROCESSING_SEMAPHORE: once_cell::sync::Lazy<Semaphore> =
    once_cell::sync::Lazy::new(|| Semaphore::new(MAX_CONCURRENT_PROCESSING));

/// Audio effect settings for pitch and tempo modifications
#[derive(Debug, Clone)]
pub struct AudioEffectSettings {
    pub pitch_semitones: i8, // -12 to +12 semitones
    pub tempo_factor: f32,   // 0.5 to 2.0
    pub bass_gain_db: i8,    // -12 to +12 dB
    pub morph_profile: MorphProfile,
}

impl Default for AudioEffectSettings {
    fn default() -> Self {
        Self {
            pitch_semitones: 0,
            tempo_factor: 1.0,
            bass_gain_db: 0,
            morph_profile: MorphProfile::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MorphProfile {
    None,
    Soft,
    Aggressive,
    Lofi,
    Wide,
}

impl std::str::FromStr for MorphProfile {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "soft" => MorphProfile::Soft,
            "aggressive" => MorphProfile::Aggressive,
            "lofi" => MorphProfile::Lofi,
            "wide" => MorphProfile::Wide,
            _ => MorphProfile::None,
        })
    }
}

impl MorphProfile {
    pub fn parse(s: &str) -> Self {
        s.parse().unwrap_or(MorphProfile::None)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            MorphProfile::None => "none",
            MorphProfile::Soft => "soft",
            MorphProfile::Aggressive => "aggressive",
            MorphProfile::Lofi => "lofi",
            MorphProfile::Wide => "wide",
        }
    }
}

/// Audio effect session for tracking modifications
#[derive(Debug, Clone)]
pub struct AudioEffectSession {
    pub id: String,
    pub user_id: i64,
    pub original_file_path: String,
    pub current_file_path: String,
    pub telegram_file_id: Option<String>,
    pub original_message_id: i32,
    pub title: String,
    pub duration: u32,
    pub pitch_semitones: i8,
    pub tempo_factor: f32,
    pub bass_gain_db: i8,
    pub morph_profile: MorphProfile,
    pub version: u32,
    pub processing: bool,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl AudioEffectSession {
    /// Create a new audio effect session
    pub fn new(
        id: String,
        user_id: i64,
        original_file_path: String,
        original_message_id: i32,
        title: String,
        duration: u32,
    ) -> Self {
        let now = Utc::now();
        let expires_at = now + Duration::hours(24);

        Self {
            id,
            user_id,
            original_file_path: original_file_path.clone(),
            current_file_path: original_file_path,
            telegram_file_id: None,
            original_message_id,
            title,
            duration,
            pitch_semitones: 0,
            tempo_factor: 1.0,
            bass_gain_db: 0,
            morph_profile: MorphProfile::None,
            version: 0,
            processing: false,
            created_at: now,
            expires_at,
        }
    }

    /// Get the current settings
    pub fn settings(&self) -> AudioEffectSettings {
        AudioEffectSettings {
            pitch_semitones: self.pitch_semitones,
            tempo_factor: self.tempo_factor,
            bass_gain_db: self.bass_gain_db,
            morph_profile: self.morph_profile,
        }
    }

    /// Check if session has expired
    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }
}

/// Error types for audio effects
#[derive(Debug, thiserror::Error)]
pub enum AudioEffectError {
    #[error("FFmpeg processing failed: {0}")]
    FFmpegError(String),

    #[error("Invalid pitch value: {0} (must be -12 to +12)")]
    InvalidPitch(i8),

    #[error("Invalid tempo value: {0} (must be 0.5 to 2.0)")]
    InvalidTempo(f32),

    #[error("Invalid bass gain value: {0} (must be -12 to +12 dB)")]
    InvalidBass(i8),

    #[error("Invalid morph profile: {0}")]
    InvalidMorph(String),

    #[error("File not found: {0}")]
    FileNotFound(String),

    #[error("Session expired or not found")]
    SessionNotFound,

    #[error("Output file too large: {0:.1} MB (max: {1} MB)")]
    FileTooLarge(f64, f64),

    #[error("Session is currently processing")]
    SessionProcessing,

    #[error("Disk space check failed: {0}")]
    DiskSpaceError(String),
}

/// Validate audio effect settings
pub fn validate_settings(settings: &AudioEffectSettings) -> Result<(), AudioEffectError> {
    if settings.pitch_semitones < -12 || settings.pitch_semitones > 12 {
        return Err(AudioEffectError::InvalidPitch(settings.pitch_semitones));
    }

    if settings.tempo_factor < 0.5 || settings.tempo_factor > 2.0 {
        return Err(AudioEffectError::InvalidTempo(settings.tempo_factor));
    }

    if settings.bass_gain_db < -12 || settings.bass_gain_db > 12 {
        return Err(AudioEffectError::InvalidBass(settings.bass_gain_db));
    }

    // Morph profile always valid (enum), but keep hook for future validation
    Ok(())
}

/// Calculate pitch ratio from semitones
/// Formula: ratio = 2^(semitones/12)
fn calculate_pitch_ratio(semitones: i8) -> f32 {
    2_f32.powf(semitones as f32 / 12.0)
}

/// Build atempo filter chain for tempo changes
/// atempo filter only supports 0.5 to 2.0 range, so we chain multiple filters for extreme values
fn build_atempo_filter(tempo: f32) -> String {
    if (0.5..=2.0).contains(&tempo) {
        format!("atempo={}", tempo)
    } else if tempo > 2.0 {
        // Chain multiple atempo filters for tempo > 2.0
        let mut filters = Vec::new();
        let mut remaining = tempo;
        while remaining > 2.0 {
            filters.push("atempo=2.0".to_string());
            remaining /= 2.0;
        }
        filters.push(format!("atempo={}", remaining));
        filters.join(",")
    } else {
        // Chain multiple atempo filters for tempo < 0.5
        let mut filters = Vec::new();
        let mut remaining = tempo;
        while remaining < 0.5 {
            filters.push("atempo=0.5".to_string());
            remaining /= 0.5;
        }
        filters.push(format!("atempo={}", remaining));
        filters.join(",")
    }
}

/// Build audio filter string for FFmpeg
fn build_audio_filter(settings: &AudioEffectSettings) -> String {
    let pitch_changed = settings.pitch_semitones != 0;
    let tempo_changed = (settings.tempo_factor - 1.0).abs() > 0.01;
    let bass_changed = settings.bass_gain_db != 0;

    // Base filter for pitch/tempo
    let core_filter = match (pitch_changed, tempo_changed) {
        (false, false) => "acopy".to_string(),
        (true, false) => {
            let pitch_ratio = calculate_pitch_ratio(settings.pitch_semitones);
            format!("rubberband=pitch={}:tempo=1", pitch_ratio)
        }
        (false, true) => build_atempo_filter(settings.tempo_factor),
        (true, true) => {
            let pitch_ratio = calculate_pitch_ratio(settings.pitch_semitones);
            format!(
                "rubberband=pitch={}:tempo={}",
                pitch_ratio, settings.tempo_factor
            )
        }
    };

    if !bass_changed {
        // apply morph profile even if bass not changed
        return append_morph_filter(core_filter, settings.morph_profile);
    }

    // Add bass shelf using ffmpeg bass filter (g in dB, f cutoff Hz, w width)
    let bass_filter = format!("bass=g={}:f=110:w=1.0", settings.bass_gain_db);

    let combined = if core_filter == "acopy" {
        bass_filter
    } else {
        format!("{},{}", core_filter, bass_filter)
    };

    append_morph_filter(combined, settings.morph_profile)
}

/// Append morph preset filters to the chain
fn append_morph_filter(base: String, profile: MorphProfile) -> String {
    let preset = match profile {
        MorphProfile::None => return base,
        MorphProfile::Soft => "acompressor=threshold=-16dB:ratio=2:attack=5:release=50, aecho=0.4:0.6:20:0.15, loudnorm=I=-16:TP=-2:LRA=11",
        MorphProfile::Aggressive => "acompressor=threshold=-18dB:ratio=4:attack=3:release=80, acrusher=bits=12:mix=0.08, aecho=0.7:0.8:40:0.25, loudnorm=I=-14:TP=-1.5:LRA=9, aresample=44100",
        MorphProfile::Lofi => "aresample=22050, acrusher=bits=10:mix=0.2, aecho=0.6:0.6:45:0.2, loudnorm=I=-18:TP=-2:LRA=8",
        MorphProfile::Wide => "aecho=0.8:0.9:60:0.3, acompressor=threshold=-14dB:ratio=2.5:attack=8:release=60, extrastereo=m=2.5, aresample=48000",
    };

    if base == "acopy" {
        preset.to_string()
    } else {
        format!("{},{}", base, preset)
    }
}

/// Apply audio effects to a file using FFmpeg
pub async fn apply_audio_effects(
    input_path: &str,
    output_path: &str,
    settings: &AudioEffectSettings,
) -> Result<(), AppError> {
    // Validate settings
    validate_settings(settings)?;

    // Check if input file exists
    if !std::path::Path::new(input_path).exists() {
        return Err(AppError::AudioEffect(AudioEffectError::FileNotFound(
            input_path.to_string(),
        )));
    }

    // Build FFmpeg filter
    let filter = build_audio_filter(settings);

    log::info!(
        "Applying audio effects: pitch={} semitones, tempo={}x, bass={} dB, morph={}, filter={}",
        settings.pitch_semitones,
        settings.tempo_factor,
        settings.bass_gain_db,
        settings.morph_profile.as_str(),
        filter
    );

    // Acquire semaphore permit for concurrent processing limit
    let _permit = PROCESSING_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| AppError::AudioEffect(AudioEffectError::FFmpegError(e.to_string())))?;

    // Execute FFmpeg in blocking task
    let input_path = input_path.to_string();
    let output_path_clone = output_path.to_string();
    tokio::task::spawn_blocking(move || {
        let output = Command::new("ffmpeg")
            .args([
                "-i",
                &input_path,
                "-af",
                &filter,
                "-q:a",
                "0",  // Highest quality VBR
                "-y", // Overwrite output
                &output_path_clone,
            ])
            .output()
            .map_err(|e| {
                AppError::AudioEffect(AudioEffectError::FFmpegError(format!(
                    "Failed to execute FFmpeg: {}",
                    e
                )))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AppError::AudioEffect(AudioEffectError::FFmpegError(
                format!("FFmpeg failed: {}", stderr),
            )));
        }

        Ok(())
    })
    .await
    .map_err(|e| {
        AppError::AudioEffect(AudioEffectError::FFmpegError(format!(
            "Task join error: {}",
            e
        )))
    })??;

    // Verify output file exists
    if !std::path::Path::new(output_path).exists() {
        return Err(AppError::AudioEffect(AudioEffectError::FFmpegError(
            "Output file not created".to_string(),
        )));
    }

    // Check output file size (Telegram limit: 50MB)
    let file_size = tokio::fs::metadata(output_path)
        .await
        .map_err(|e| {
            AppError::AudioEffect(AudioEffectError::FFmpegError(format!(
                "Failed to check file size: {}",
                e
            )))
        })?
        .len();

    let max_size = 50 * 1024 * 1024; // 50 MB
    if file_size > max_size {
        // Clean up oversized file
        let _ = tokio::fs::remove_file(output_path).await;
        return Err(AppError::AudioEffect(AudioEffectError::FileTooLarge(
            file_size as f64 / (1024.0 * 1024.0),
            50.0,
        )));
    }

    log::info!(
        "Audio effects applied successfully: {} (size: {:.2} MB)",
        output_path,
        file_size as f64 / (1024.0 * 1024.0)
    );

    Ok(())
}

/// Generate file path for original audio
pub fn get_original_file_path(session_id: &str, download_folder: &str) -> String {
    format!("{}/original-{}.mp3", download_folder, session_id)
}

/// Generate file path for modified audio
pub fn get_modified_file_path(session_id: &str, version: u32, download_folder: &str) -> String {
    format!(
        "{}/modified-{}-v{}.mp3",
        download_folder, session_id, version
    )
}

/// Cleanup expired audio effect sessions
pub async fn cleanup_expired_sessions(db_pool: Arc<DbPool>) -> Result<usize, AppError> {
    use crate::storage::db;

    let conn = db::get_connection(&db_pool)?;

    // Get expired sessions
    let expired_sessions = db::delete_expired_audio_sessions(&conn)?;

    let count = expired_sessions.len();

    // Delete files for expired sessions
    for session in expired_sessions {
        // Delete original file
        if let Err(e) = tokio::fs::remove_file(&session.original_file_path).await {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    "Failed to delete original file {}: {}",
                    session.original_file_path,
                    e
                );
            }
        }

        // Delete current file if different
        if session.current_file_path != session.original_file_path {
            if let Err(e) = tokio::fs::remove_file(&session.current_file_path).await {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!(
                        "Failed to delete current file {}: {}",
                        session.current_file_path,
                        e
                    );
                }
            }
        }
    }

    if count > 0 {
        log::info!("Cleaned up {} expired audio effect session(s)", count);
    }

    Ok(count)
}

/// Start background cleanup task for expired sessions
pub fn start_cleanup_task(db_pool: Arc<DbPool>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600)); // 1 hour
        loop {
            interval.tick().await;
            match cleanup_expired_sessions(Arc::clone(&db_pool)).await {
                Ok(count) if count > 0 => {
                    log::info!("Audio effects cleanup: removed {} session(s)", count);
                }
                Ok(_) => {} // No sessions to clean
                Err(e) => {
                    log::error!("Audio effects cleanup failed: {}", e);
                }
            }
        }
    });

    log::info!("Audio effects cleanup task started (runs every hour)");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_pitch_ratio() {
        assert_eq!(calculate_pitch_ratio(0), 1.0);
        assert!((calculate_pitch_ratio(12) - 2.0).abs() < 0.01);
        assert!((calculate_pitch_ratio(-12) - 0.5).abs() < 0.01);
        assert!((calculate_pitch_ratio(1) - 1.0595).abs() < 0.001);
    }

    #[test]
    fn test_build_atempo_filter() {
        assert_eq!(build_atempo_filter(1.0), "atempo=1");
        assert_eq!(build_atempo_filter(1.5), "atempo=1.5");
        assert_eq!(build_atempo_filter(0.75), "atempo=0.75");

        // Test chaining for > 2.0
        let result = build_atempo_filter(3.0);
        assert!(result.contains("atempo=2.0"));
        assert!(result.contains("atempo=1.5"));

        // Test chaining for < 0.5
        let result = build_atempo_filter(0.25);
        assert!(result.contains("atempo=0.5"));
    }

    #[test]
    fn test_validate_settings() {
        let valid = AudioEffectSettings {
            pitch_semitones: 5,
            tempo_factor: 1.5,
            bass_gain_db: 0,
            morph_profile: MorphProfile::None,
        };
        assert!(validate_settings(&valid).is_ok());

        let invalid_pitch = AudioEffectSettings {
            pitch_semitones: 15,
            tempo_factor: 1.0,
            bass_gain_db: 0,
            morph_profile: MorphProfile::None,
        };
        assert!(validate_settings(&invalid_pitch).is_err());

        let invalid_tempo = AudioEffectSettings {
            pitch_semitones: 0,
            tempo_factor: 3.0,
            bass_gain_db: 0,
            morph_profile: MorphProfile::None,
        };
        assert!(validate_settings(&invalid_tempo).is_err());
    }

    #[test]
    fn test_build_audio_filter() {
        // No changes
        let settings = AudioEffectSettings {
            pitch_semitones: 0,
            tempo_factor: 1.0,
            bass_gain_db: 0,
            morph_profile: MorphProfile::None,
        };
        assert_eq!(build_audio_filter(&settings), "acopy");

        // Pitch only
        let settings = AudioEffectSettings {
            pitch_semitones: 2,
            tempo_factor: 1.0,
            bass_gain_db: 0,
            morph_profile: MorphProfile::None,
        };
        let filter = build_audio_filter(&settings);
        assert!(filter.starts_with("rubberband=pitch="));
        assert!(filter.contains(":tempo=1"));

        // Tempo only
        let settings = AudioEffectSettings {
            pitch_semitones: 0,
            tempo_factor: 1.5,
            bass_gain_db: 0,
            morph_profile: MorphProfile::None,
        };
        assert_eq!(build_audio_filter(&settings), "atempo=1.5");

        // Both
        let settings = AudioEffectSettings {
            pitch_semitones: 3,
            tempo_factor: 0.8,
            bass_gain_db: 0,
            morph_profile: MorphProfile::None,
        };
        let filter = build_audio_filter(&settings);
        assert!(filter.starts_with("rubberband=pitch="));
        assert!(filter.contains(":tempo=0.8"));
    }
}
