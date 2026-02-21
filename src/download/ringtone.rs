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
    // -ss/-t must come AFTER -i to avoid seek issues; -vn strips embedded album art
    // which causes exit code 234 with -f ipod when the input has a video stream (e.g. MP3 cover art)
    let start_str = start_secs.to_string();
    let duration_str = duration.to_string();
    let output = run_with_timeout(
        Command::new("ffmpeg")
            .arg("-i")
            .arg(input)
            .arg("-ss")
            .arg(&start_str)
            .arg("-t")
            .arg(&duration_str)
            .arg("-vn")
            .arg("-c:a")
            .arg("aac")
            .arg("-b:a")
            .arg("192k")
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

    // ==================== constant / pure-logic tests ====================

    #[test]
    fn test_max_duration_constant() {
        assert_eq!(MAX_RINGTONE_DURATION_SECS, 30);
    }

    #[test]
    fn test_duration_clamped_to_max() {
        // The clamping is: duration_secs.min(MAX_RINGTONE_DURATION_SECS)
        let clamped = 60u32.min(MAX_RINGTONE_DURATION_SECS);
        assert_eq!(clamped, 30);
    }

    #[test]
    fn test_duration_under_max_not_clamped() {
        let clamped = 20u32.min(MAX_RINGTONE_DURATION_SECS);
        assert_eq!(clamped, 20);
    }

    #[test]
    fn test_duration_exactly_max_not_clamped() {
        let clamped = 30u32.min(MAX_RINGTONE_DURATION_SECS);
        assert_eq!(clamped, 30);
    }

    // ==================== FFmpeg integration tests (require ffmpeg) ====================

    #[tokio::test]
    async fn test_ringtone_creation_invalid_input() {
        let result = create_iphone_ringtone("non_existent_file.mp3", "output.m4r", 0, 30).await;
        assert!(result.is_err());
    }

    fn ffmpeg_available() -> bool {
        std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[tokio::test]
    async fn test_ringtone_creation_from_silence() {
        if !ffmpeg_available() {
            eprintln!("Skipping: ffmpeg not available");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let input_path = dir.path().join("ringtone_in.mp3");
        let output_path = dir.path().join("ringtone_out.m4r");

        // Create 2 seconds of silence as MP3
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-f",
                "lavfi",
                "-i",
                "anullsrc=r=44100:cl=stereo",
                "-t",
                "2",
                "-y",
                input_path.to_str().unwrap(),
            ])
            .status()
            .unwrap();
        assert!(status.success(), "ffmpeg setup failed");

        let result = create_iphone_ringtone(&input_path, &output_path, 0, 1).await;
        assert!(result.is_ok(), "Ringtone failed: {:?}", result.err());
        assert!(output_path.exists(), "Output .m4r file not created");
    }

    /// Regression test for exit code 234: MP3 with embedded album art should not fail.
    /// The bug was caused by -movflags +faststart conflicting with -f ipod when a video
    /// stream (cover art) was present. Fixed by adding -vn and moving -ss/-t after -i.
    #[tokio::test]
    async fn test_ringtone_with_embedded_album_art_does_not_fail() {
        if !ffmpeg_available() {
            eprintln!("Skipping: ffmpeg not available");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let silence_path = dir.path().join("silence.mp3");
        let cover_path = dir.path().join("cover.jpg");
        let input_with_art = dir.path().join("with_art.mp3");
        let output_path = dir.path().join("out.m4r");

        // Generate a minimal 1x1 JPEG cover image
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-f",
                "lavfi",
                "-i",
                "color=red:size=1x1",
                "-frames:v",
                "1",
                "-y",
                cover_path.to_str().unwrap(),
            ])
            .status()
            .unwrap();
        if !status.success() {
            eprintln!("Skipping: could not generate cover art");
            return;
        }

        // Generate 2 seconds of silence
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-f",
                "lavfi",
                "-i",
                "anullsrc=r=44100:cl=stereo",
                "-t",
                "2",
                "-y",
                silence_path.to_str().unwrap(),
            ])
            .status()
            .unwrap();
        assert!(status.success());

        // Embed the cover art into the MP3 (creates a video stream, the source of exit 234)
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-i",
                silence_path.to_str().unwrap(),
                "-i",
                cover_path.to_str().unwrap(),
                "-map",
                "0:a",
                "-map",
                "1:v",
                "-c:a",
                "copy",
                "-c:v",
                "copy",
                "-id3v2_version",
                "3",
                "-y",
                input_with_art.to_str().unwrap(),
            ])
            .status()
            .unwrap();
        assert!(status.success(), "embedding album art failed");

        // This should NOT fail with exit code 234 thanks to -vn
        let result = create_iphone_ringtone(&input_with_art, &output_path, 0, 1).await;
        assert!(
            result.is_ok(),
            "Ringtone from MP3 with album art failed: {:?}",
            result.err()
        );
        assert!(output_path.exists());
    }

    #[tokio::test]
    async fn test_ringtone_creation_valid() {
        if !ffmpeg_available() {
            eprintln!("Skipping: ffmpeg not available");
            return;
        }
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
