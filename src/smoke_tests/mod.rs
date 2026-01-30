//! Smoke tests module for health checks and CI verification.
//!
//! This module provides end-to-end tests that verify the bot's core functionality:
//! - Tool availability (ffmpeg, ffprobe, yt-dlp)
//! - Cookies validation
//! - Metadata extraction from YouTube
//! - Audio download (MP3)
//! - Video download (MP4)
//!
//! # Usage
//!
//! ## In CI (GitHub Actions)
//! ```bash
//! cargo test --test smoke_test -- --nocapture
//! ```
//!
//! ## As production health check
//! The scheduler runs tests every hour and sends alerts on failure.
//!
//! # Configuration
//!
//! - `HEALTH_CHECK_ENABLED`: Enable hourly health checks (default: true)
//! - `HEALTH_CHECK_INTERVAL_SECS`: Interval between checks (default: 3600)

mod results;
pub mod runner;
mod scheduler;
mod test_cases;
mod validators;

pub use results::{SmokeTestReport, SmokeTestResult, SmokeTestStatus};
pub use runner::{run_all_smoke_tests, SmokeTestConfig};
pub use scheduler::{start_health_check_scheduler, HealthCheckScheduler};
pub use test_cases::{
    test_audio_download, test_cookies_validation, test_ffmpeg_toolchain, test_metadata_extraction, test_video_download,
};
pub use validators::{
    is_ffmpeg_available, is_ffprobe_available, is_ytdlp_available, validate_audio_file, validate_video_file,
    AudioFileValidation, VideoFileValidation,
};

/// Default test URL - "Me at the zoo" (first YouTube video, ~19 seconds)
/// This is the most stable video on YouTube for testing purposes.
pub const DEFAULT_TEST_URL: &str = "https://www.youtube.com/watch?v=jNQXAC9IVRw";

/// Default timeout for individual tests in seconds
pub const DEFAULT_TEST_TIMEOUT_SECS: u64 = 180;

/// Production health check timeout (shorter to avoid blocking)
pub const PRODUCTION_TEST_TIMEOUT_SECS: u64 = 120;
