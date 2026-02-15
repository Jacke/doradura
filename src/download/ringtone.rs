use crate::core::error::AppError;
use crate::core::process::{run_with_timeout, FFMPEG_TIMEOUT};
use crate::download::error::DownloadError;
use std::path::Path;
use tokio::process::Command;

/// Maximum duration for an iPhone ringtone (30 seconds)
pub const MAX_RINGTONE_DURATION_SECS: u32 = 30;

/// Converts an audio file to an iPhone ringtone (.m4r format)
///
/// # Arguments
/// * `input_path` - Path to the source audio/video file
/// * `output_path` - Path to save the converted .m4r file
/// * `start_secs` - Start time in seconds
/// * `duration_secs` - Duration in seconds (clamped to MAX_RINGTONE_DURATION_SECS)
///
/// # Returns
/// * `Ok(())` on success
/// * `Err(AppError)` on failure
pub async fn create_iphone_ringtone<P: AsRef<Path>>(
    input_path: P,
    output_path: P,
    start_secs: u32,
    duration_secs: u32,
) -> Result<(), AppError> {
    let input = input_path.as_ref();
    let output = output_path.as_ref();

    // Clamp duration to iOS limit (usually 30-40s, 30s is safe)
    let duration = duration_secs.min(MAX_RINGTONE_DURATION_SECS);

    log::info!(
        "🔔 Creating ringtone: {:?} -> {:?} (start: {}s, duration: {}s)",
        input,
        output,
        start_secs,
        duration
    );

    // FFmpeg command to convert to AAC and package as M4A (which is what .m4r is)
    // -i input -ss start -t duration -c:a aac -b:a 192k -movflags +faststart -y output
    let start_str = start_secs.to_string();
    let duration_str = duration.to_string();
    let output = run_with_timeout(
        Command::new("ffmpeg")
            .arg("-ss")
            .arg(&start_str)
            .arg("-t")
            .arg(&duration_str)
            .arg("-i")
            .arg(input)
            .arg("-c:a")
            .arg("aac")
            .arg("-b:a")
            .arg("192k")
            .arg("-movflags")
            .arg("+faststart")
            .arg("-f")
            .arg("ipod")
            .arg("-y")
            .arg(output),
        FFMPEG_TIMEOUT,
    )
    .await?;

    if !output.status.success() {
        return Err(AppError::Download(DownloadError::Ffmpeg(format!(
            "FFmpeg failed with exit status: {:?}",
            output.status.code()
        ))));
    }

    log::info!("✅ Ringtone created successfully: {:?}", input_path.as_ref());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_max_duration_constant() {
        assert_eq!(MAX_RINGTONE_DURATION_SECS, 30);
    }

    #[tokio::test]
    async fn test_ringtone_creation_invalid_input() {
        let result = create_iphone_ringtone("non_existent_file.mp3", "output.m4r", 0, 30).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ringtone_creation_valid() {
        // Create a dummy input file (1 second of silence) using ffmpeg
        let input_path = "test_input.mp3";
        let output_path = "test_output.m4r";

        let setup_status = std::process::Command::new("ffmpeg")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("anullsrc=r=44100:cl=stereo")
            .arg("-t")
            .arg("1")
            .arg("-y")
            .arg(input_path)
            .status();

        if setup_status.is_err() || !setup_status.unwrap().success() {
            eprintln!("Skipping test: ffmpeg not available for setup");
            return;
        }

        // Run the conversion
        let result = create_iphone_ringtone(input_path, output_path, 0, 1).await;

        // Assert success
        assert!(result.is_ok(), "Ringtone creation failed: {:?}", result.err());
        assert!(Path::new(output_path).exists(), "Output file does not exist");

        // Cleanup
        let _ = fs::remove_file(input_path);
        let _ = fs::remove_file(output_path);
    }
}
