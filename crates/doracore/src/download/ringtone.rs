use crate::core::error::AppError;
use crate::core::process::{run_with_timeout, FFMPEG_TIMEOUT};
use crate::download::error::DownloadError;
use std::path::Path;
use tokio::process::Command;

/// Maximum duration for an iPhone ringtone (30 seconds)
pub const MAX_RINGTONE_DURATION_SECS: u32 = 30;
/// Alias for iPhone max duration
pub const MAX_IPHONE_DURATION_SECS: u32 = 30;
/// Maximum duration for an Android ringtone (40 seconds)
pub const MAX_ANDROID_DURATION_SECS: u32 = 40;

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

/// Converts an audio file to an Android ringtone (.mp3 format, 192k, max 40 sec)
pub async fn create_android_ringtone<P: AsRef<Path>>(
    input_path: P,
    output_path: P,
    start_secs: u32,
    duration_secs: u32,
) -> Result<(), AppError> {
    let input = input_path.as_ref();
    let output = output_path.as_ref();

    let duration = duration_secs.min(MAX_ANDROID_DURATION_SECS);

    log::info!(
        "🔔 Creating Android ringtone: {:?} -> {:?} (start: {}s, duration: {}s)",
        input,
        output,
        start_secs,
        duration
    );

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
            .arg("libmp3lame")
            .arg("-b:a")
            .arg("192k")
            .arg("-f")
            .arg("mp3")
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

    log::info!("✅ Android ringtone created successfully: {:?}", input_path.as_ref());
    Ok(())
}

/// Sanitize a track title for use as a filename.
/// Replaces spaces with underscores, removes characters not in [A-Za-z0-9_.-],
/// and limits to 60 characters.
pub fn sanitize_filename(title: &str) -> String {
    let replaced: String = title
        .chars()
        .map(|c| if c == ' ' { '_' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '.')
        .collect();
    let trimmed = replaced.trim_matches(['_', '-', '.']);
    if trimmed.is_empty() {
        return "ringtone".to_string();
    }
    let truncated: String = trimmed.chars().take(60).collect();
    truncated.trim_end_matches(['_', '-', '.']).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs_err as fs;

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

    // ==================== Android constants ====================

    #[test]
    fn test_android_max_duration_constant() {
        assert_eq!(MAX_ANDROID_DURATION_SECS, 40);
    }

    #[test]
    fn test_iphone_max_duration_constant() {
        assert_eq!(MAX_IPHONE_DURATION_SECS, 30);
    }

    #[test]
    fn test_android_max_greater_than_iphone_max() {
        const { assert!(MAX_ANDROID_DURATION_SECS > MAX_IPHONE_DURATION_SECS) };
    }

    #[test]
    fn test_android_duration_clamped_to_40() {
        let clamped = 60u32.min(MAX_ANDROID_DURATION_SECS);
        assert_eq!(clamped, 40);
    }

    #[test]
    fn test_android_duration_under_40_not_clamped() {
        let clamped = 30u32.min(MAX_ANDROID_DURATION_SECS);
        assert_eq!(clamped, 30);
    }

    // ==================== FFmpeg integration tests (require ffmpeg) ====================

    // ==================== sanitize_filename tests ====================

    #[test]
    fn test_sanitize_filename_basic() {
        assert_eq!(sanitize_filename("Hello World"), "Hello_World");
    }

    #[test]
    fn test_sanitize_filename_special_chars() {
        // colon and parens are removed; spaces become underscores
        assert_eq!(sanitize_filename("Song: (feat. Artist)"), "Song_feat._Artist");
    }

    #[test]
    fn test_sanitize_filename_empty() {
        assert_eq!(sanitize_filename(""), "ringtone");
    }

    #[test]
    fn test_sanitize_filename_only_special() {
        assert_eq!(sanitize_filename("!!!"), "ringtone");
    }

    #[test]
    fn test_sanitize_filename_truncates_at_60() {
        let long_title = "A".repeat(80);
        let result = sanitize_filename(&long_title);
        assert!(result.len() <= 60, "Length {} exceeds 60", result.len());
    }

    #[test]
    fn test_sanitize_filename_hyphens_and_dots() {
        assert_eq!(sanitize_filename("Track-01.mp3"), "Track-01.mp3");
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

    // ==================== create_android_ringtone tests ====================

    #[tokio::test]
    async fn test_android_ringtone_creation_invalid_input() {
        let result = create_android_ringtone("non_existent_file.mp3", "output.mp3", 0, 30).await;
        assert!(result.is_err(), "Expected error for nonexistent input");
    }

    #[tokio::test]
    async fn test_android_ringtone_creation_from_silence() {
        if !ffmpeg_available() {
            eprintln!("Skipping: ffmpeg not available");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let input_path = dir.path().join("android_in.mp3");
        let output_path = dir.path().join("android_out.mp3");

        // Create 5 seconds of silence
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-f",
                "lavfi",
                "-i",
                "anullsrc=r=44100:cl=stereo",
                "-t",
                "5",
                "-y",
                input_path.to_str().unwrap(),
            ])
            .status()
            .unwrap();
        assert!(status.success(), "ffmpeg setup failed");

        let result = create_android_ringtone(&input_path, &output_path, 0, 5).await;
        assert!(result.is_ok(), "Android ringtone failed: {:?}", result.err());
        assert!(output_path.exists(), "Output .mp3 file not created");

        // File should have non-zero size
        let meta = fs::metadata(&output_path).unwrap();
        assert!(meta.len() > 0, "Output .mp3 file is empty");
    }

    #[tokio::test]
    async fn test_android_ringtone_duration_clamped_to_40s() {
        if !ffmpeg_available() {
            eprintln!("Skipping: ffmpeg not available");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let input_path = dir.path().join("android_long_in.mp3");
        let output_path = dir.path().join("android_long_out.mp3");

        // Create 60 seconds of silence
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-f",
                "lavfi",
                "-i",
                "anullsrc=r=44100:cl=stereo",
                "-t",
                "60",
                "-y",
                input_path.to_str().unwrap(),
            ])
            .status()
            .unwrap();
        assert!(status.success(), "ffmpeg setup failed");

        // Request 60s — should be clamped to 40s
        let result = create_android_ringtone(&input_path, &output_path, 0, 60).await;
        assert!(result.is_ok(), "Android ringtone clamped failed: {:?}", result.err());
        assert!(output_path.exists(), "Output not created");

        // Check duration with ffprobe
        let probe = std::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "csv=p=0",
                output_path.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        if probe.status.success() {
            let duration_str = String::from_utf8_lossy(&probe.stdout).trim().to_string();
            if let Ok(dur) = duration_str.parse::<f64>() {
                assert!(
                    dur <= 41.0,
                    "Duration {:.1}s exceeds 40s (MAX_ANDROID_DURATION_SECS)",
                    dur
                );
            }
        }
    }

    #[tokio::test]
    async fn test_android_ringtone_output_is_mp3() {
        if !ffmpeg_available() {
            eprintln!("Skipping: ffmpeg not available");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let input_path = dir.path().join("android_fmt_in.mp3");
        let output_path = dir.path().join("android_fmt_out.mp3");

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
        assert!(status.success());

        create_android_ringtone(&input_path, &output_path, 0, 2).await.unwrap();

        // Check format with ffprobe
        let probe = std::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=format_name",
                "-of",
                "csv=p=0",
                output_path.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        if probe.status.success() {
            let fmt = String::from_utf8_lossy(&probe.stdout).to_lowercase();
            assert!(fmt.contains("mp3"), "Expected mp3 format, got: {}", fmt.trim());
        }
    }

    /// Regression: MP3 with embedded album art must not fail for Android ringtone either
    #[tokio::test]
    async fn test_android_ringtone_with_embedded_album_art() {
        if !ffmpeg_available() {
            eprintln!("Skipping: ffmpeg not available");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let silence_path = dir.path().join("silence.mp3");
        let cover_path = dir.path().join("cover.jpg");
        let input_with_art = dir.path().join("with_art.mp3");
        let output_path = dir.path().join("android_out.mp3");

        // Create 1×1 cover image
        let s = std::process::Command::new("ffmpeg")
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
        if !s.success() {
            eprintln!("Skipping: cannot generate cover");
            return;
        }

        // Create 2s silence
        let s = std::process::Command::new("ffmpeg")
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
        assert!(s.success());

        // Embed cover art
        let s = std::process::Command::new("ffmpeg")
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
        assert!(s.success(), "embedding album art failed");

        let result = create_android_ringtone(&input_with_art, &output_path, 0, 1).await;
        assert!(
            result.is_ok(),
            "Android ringtone from MP3 with album art failed: {:?}",
            result.err()
        );
        assert!(output_path.exists());
    }
}
