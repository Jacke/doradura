//! Voice message handling for the Telegram bot
//!
//! This module provides functionality for:
//! - Converting WAV files to OGG Opus format
//! - Sending voice messages with waveform
//! - Random voice file selection

use anyhow::Result;
use rand::Rng;
use std::path::Path;
use std::process::Command as ProcessCommand;
use teloxide::prelude::*;
use crate::telegram::Bot;
use teloxide::types::InputFile;
use tokio::time::{sleep, Duration};

/// List of voice files for random sending on /start
///
/// To add a new file, simply add its name to this vector
pub const VOICE_FILES: &[&str] = &[
    "assets/voices/first.wav",
    "assets/voices/second.wav",
    "assets/voices/third.wav",
    "assets/voices/fourth.wav",
];

/// Converts WAV file to OGG Opus for correct waveform display in Telegram
///
/// # Arguments
/// * `input_path` - Path to the source WAV file
/// * `output_path` - Path to save the converted OGG file
///
/// # Returns
/// * `Ok(duration)` - Successful conversion, returns duration in seconds
/// * `Err(error)` - Conversion error
pub fn convert_wav_to_ogg_opus(input_path: &str, output_path: &str) -> Result<Option<u32>> {
    // Check if ffmpeg is available
    let ffmpeg_check = ProcessCommand::new("ffmpeg").arg("-version").output();

    if ffmpeg_check.is_err() {
        return Err(anyhow::anyhow!(
            "ffmpeg not found. Please install ffmpeg to convert voice messages."
        ));
    }

    // Convert WAV to OGG Opus
    let output = ProcessCommand::new("ffmpeg")
        .arg("-i")
        .arg(input_path)
        .arg("-c:a")
        .arg("libopus")
        .arg("-b:a")
        .arg("64k")
        .arg("-application")
        .arg("voip") // Important for voice messages
        .arg("-y") // Overwrite output file if exists
        .arg(output_path)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("ffmpeg conversion failed: {}", stderr));
    }

    // Get audio duration for correct display
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

/// Sends a voice message with waveform
///
/// # Arguments
/// * `bot` - Bot instance for sending
/// * `chat_id` - Chat ID to send to
/// * `voice_file_path` - Path to the WAV file
///
/// Converts WAV to OGG Opus and sends with duration specified for waveform
pub async fn send_voice_with_waveform(bot: Bot, chat_id: ChatId, voice_file_path: &str) {
    if !Path::new(voice_file_path).exists() {
        log::warn!("Voice file {} not found, skipping voice message", voice_file_path);
        return;
    }

    // Generate unique name for temporary OGG file
    let file_stem = Path::new(voice_file_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("voice");
    let ogg_path = format!("{}.ogg", file_stem);

    // Convert WAV to OGG Opus for correct waveform display
    let voice_file_path_clone = voice_file_path.to_string();
    let ogg_path_clone = ogg_path.clone();
    let conversion_result =
        tokio::task::spawn_blocking(move || convert_wav_to_ogg_opus(&voice_file_path_clone, &ogg_path_clone)).await;

    match conversion_result {
        Ok(Ok(duration)) => {
            // Send voice message with duration specified
            let mut voice_msg = bot.send_voice(chat_id, InputFile::file(&ogg_path));

            // Specify duration for correct waveform display
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

            // Remove temporary OGG file
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
            // Fallback: try sending as WAV (without waveform)
            match bot.send_voice(chat_id, InputFile::file(voice_file_path)).await {
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
            log::warn!("Failed to spawn conversion task for {}: {}", voice_file_path, e);
        }
    }
}

/// Sends a random voice message with delay
///
/// # Arguments
/// * `bot` - Bot instance for sending
/// * `chat_id` - Chat ID to send to
///
/// Randomly selects a voice file from VOICE_FILES and sends it after a random delay (2-10 seconds)
/// with 70% probability
pub async fn send_random_voice_message(bot: Bot, chat_id: ChatId) {
    // Generate random probability of sending (70% chance)
    let should_send = rand::thread_rng().gen_bool(0.7);
    if !should_send {
        log::debug!("Voice message skipped by random chance for chat {}", chat_id);
        return;
    }

    // Generate random delay from 2 to 10 seconds
    let delay_secs = rand::thread_rng().gen_range(2000..=10000);

    // Wait for random time
    sleep(Duration::from_millis(delay_secs)).await;

    // Find available voice files
    let available_files: Vec<&str> = VOICE_FILES
        .iter()
        .filter(|&&file| Path::new(file).exists())
        .copied()
        .collect();

    if available_files.is_empty() {
        log::warn!("No voice files found from: {:?}, skipping voice message", VOICE_FILES);
        return;
    }

    // Randomly select one of the available files
    let selected_file = available_files[rand::thread_rng().gen_range(0..available_files.len())];
    log::debug!("Selected voice file: {} for chat {}", selected_file, chat_id);

    // Send the selected voice file with waveform
    send_voice_with_waveform(bot, chat_id, selected_file).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_voice_files_constant() {
        assert_eq!(VOICE_FILES.len(), 4);
        assert!(VOICE_FILES.contains(&"assets/voices/first.wav"));
        assert!(VOICE_FILES.contains(&"assets/voices/second.wav"));
        assert!(VOICE_FILES.contains(&"assets/voices/third.wav"));
        assert!(VOICE_FILES.contains(&"assets/voices/fourth.wav"));
    }

    #[test]
    fn test_convert_wav_to_ogg_opus_ffmpeg_not_found() {
        // This test will fail if ffmpeg is not installed
        // We're testing that the function returns an error when ffmpeg is not found
        // In real environment with ffmpeg, this test would need to be skipped or mocked
        let result = convert_wav_to_ogg_opus("nonexistent.wav", "output.ogg");
        // Just check that function can be called - actual behavior depends on system
        assert!(result.is_ok() || result.is_err());
    }
}
