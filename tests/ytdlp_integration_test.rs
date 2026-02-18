use std::path::PathBuf;
/// Integration test for verifying yt-dlp functionality
///
/// This test checks:
/// - Whether yt-dlp is installed
/// - Whether video downloading works
/// - Whether cookies work (if configured)
/// - Whether errors are handled correctly
///
/// Run: cargo test --test ytdlp_integration_test -- --nocapture --test-threads=1
/// Run specific test: cargo test --test ytdlp_integration_test test_ytdlp_download_with_metadata -- --nocapture
use std::process::{Command, Stdio};
use std::time::Duration;
use std::{env, fs};

/// Checks whether a command exists in PATH
fn command_exists(bin: &str) -> bool {
    Command::new("bash")
        .arg("-lc")
        .arg(format!("command -v {} >/dev/null 2>&1", bin))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Returns the path to the temporary directory for tests
fn get_test_downloads_dir() -> PathBuf {
    let tmp_dir = env::temp_dir().join("doradura_ytdlp_tests");
    let _ = fs::create_dir_all(&tmp_dir);
    tmp_dir
}

/// Cleans up the temporary directory after a test
fn cleanup_test_dir(dir: &PathBuf) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Gets the path to the cookies file from an environment variable
fn get_cookies_file() -> Option<String> {
    env::var("YTDL_COOKIES_FILE").ok()
}

/// Gets the browser name for cookies from an environment variable
fn get_cookies_browser() -> Option<String> {
    env::var("YTDL_COOKIES_BROWSER").ok()
}

/// Test 1: Check yt-dlp and ffmpeg installation
#[test]
fn test_ytdlp_installed() {
    println!("=== Checking yt-dlp installation ===");

    let ytdlp_exists = command_exists("yt-dlp");
    let ffmpeg_exists = command_exists("ffmpeg");
    let ffprobe_exists = command_exists("ffprobe");

    println!("✓ yt-dlp: {}", if ytdlp_exists { "installed" } else { "NOT INSTALLED" });
    println!(
        "✓ ffmpeg: {}",
        if ffmpeg_exists { "installed" } else { "NOT INSTALLED" }
    );
    println!(
        "✓ ffprobe: {}",
        if ffprobe_exists { "installed" } else { "NOT INSTALLED" }
    );

    if !ytdlp_exists {
        println!("\n❌ ERROR: yt-dlp is not installed!");
        println!("Install with: pip3 install yt-dlp");
    }

    if !ffmpeg_exists || !ffprobe_exists {
        println!("\n❌ ERROR: ffmpeg/ffprobe is not installed!");
        println!("Install with: brew install ffmpeg (macOS) or apt install ffmpeg (Linux)");
    }

    assert!(ytdlp_exists, "yt-dlp must be installed");
    assert!(ffmpeg_exists, "ffmpeg must be installed");
    assert!(ffprobe_exists, "ffprobe must be installed");
}

/// Test 2: Check yt-dlp version
#[test]
fn test_ytdlp_version() {
    if !command_exists("yt-dlp") {
        println!("⚠️  yt-dlp is not installed, skipping test");
        return;
    }

    println!("=== Checking yt-dlp version ===");

    let output = Command::new("yt-dlp")
        .arg("--version")
        .output()
        .expect("Failed to run yt-dlp --version");

    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    println!("✓ yt-dlp version: {}", version);

    assert!(!version.is_empty(), "Failed to get yt-dlp version");
}

/// Test 3: Check cookies configuration
#[test]
fn test_cookies_configuration() {
    println!("=== Checking cookies configuration ===");

    let cookies_file = get_cookies_file();
    let cookies_browser = get_cookies_browser();

    match (&cookies_file, &cookies_browser) {
        (Some(file), _) => {
            println!("✓ Using cookies file: {}", file);

            // Check if file exists
            if std::path::Path::new(file).exists() {
                println!("✓ File exists");

                // Check file size
                if let Ok(metadata) = fs::metadata(file) {
                    println!("✓ File size: {} bytes", metadata.len());
                    assert!(metadata.len() > 0, "Cookies file is empty");
                }
            } else {
                println!("❌ ERROR: Cookies file not found at path: {}", file);
                panic!("Cookies file does not exist");
            }
        }
        (None, Some(browser)) => {
            println!("✓ Using browser for cookies: {}", browser);
            println!("⚠️  WARNING: On macOS Full Disk Access is required to extract cookies from browser");
            println!("   It is recommended to use a cookies file instead of browser");
        }
        (None, None) => {
            println!("❌ ERROR: Cookies are not configured!");
            println!("\nTo work with YouTube you need to configure cookies:");
            println!("1. Export cookies from your browser to a file");
            println!("2. Set environment variable: export YTDL_COOKIES_FILE=/path/to/cookies.txt");
            println!("3. Or use browser: export YTDL_COOKIES_BROWSER=chrome");
            println!("\nSee documentation: MACOS_COOKIES_FIX.md");

            // This is a warning, not a failure
            eprintln!("\n⚠️  Without cookies most YouTube videos will not download!");
        }
    }
}

/// Test 4: Check metadata retrieval from a public video
#[test]
#[ignore] // Requires network connection
fn test_ytdlp_get_metadata() {
    if !command_exists("yt-dlp") {
        println!("⚠️  yt-dlp is not installed, skipping test");
        return;
    }

    println!("=== Checking video metadata retrieval ===");

    // Using a short public video
    let test_url = "https://www.youtube.com/watch?v=jNQXAC9IVRw"; // "Me at the zoo" - first YouTube video

    let mut cmd = Command::new("yt-dlp");
    cmd.args(["--get-title", "--no-playlist"]);

    // Add cookies if available
    if let Some(cookies_file) = get_cookies_file() {
        cmd.args(["--cookies", &cookies_file]);
        println!("✓ Using cookies file: {}", cookies_file);
    } else if let Some(browser) = get_cookies_browser() {
        cmd.args(["--cookies-from-browser", &browser]);
        println!("✓ Using browser for cookies: {}", browser);
    }

    cmd.arg(test_url);

    let output = cmd.output().expect("Failed to run yt-dlp");

    if output.status.success() {
        let title = String::from_utf8_lossy(&output.stdout).trim().to_string();
        println!("✓ Retrieved title: {}", title);
        assert!(!title.is_empty(), "Title must not be empty");
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        println!("❌ ERROR retrieving metadata:");
        println!("{}", stderr);

        // Analyze the error
        if stderr.contains("Please sign in") || stderr.contains("cookies") {
            println!("\n💡 Solution: Configure cookies (see MACOS_COOKIES_FIX.md)");
        }
        if stderr.contains("PO Token") {
            println!("\n💡 Solution: Update yt-dlp to the latest version");
        }

        panic!("Failed to retrieve video metadata");
    }
}

/// Test 5: Audio download with success verification
#[test]
#[ignore] // Requires network connection
fn test_ytdlp_download_audio() {
    if !command_exists("yt-dlp") || !command_exists("ffmpeg") {
        println!("⚠️  yt-dlp or ffmpeg is not installed, skipping test");
        return;
    }

    println!("=== Audio download test ===");

    let tmp_dir = get_test_downloads_dir();
    let output_file = tmp_dir.join("test_audio.mp3");

    // Clean up old files
    cleanup_test_dir(&tmp_dir);

    // Using a short public video
    let test_url = "https://www.youtube.com/watch?v=jNQXAC9IVRw"; // ~19 seconds

    let mut cmd = Command::new("yt-dlp");
    cmd.args([
        "-o",
        output_file.to_str().expect("Invalid UTF-8 in output file path"),
        "--extract-audio",
        "--audio-format",
        "mp3",
        "--audio-quality",
        "0",
        "--no-playlist",
    ]);

    // Add cookies if available
    if let Some(cookies_file) = get_cookies_file() {
        cmd.args(["--cookies", &cookies_file]);
        println!("✓ Using cookies file: {}", cookies_file);
    } else if let Some(browser) = get_cookies_browser() {
        cmd.args(["--cookies-from-browser", &browser]);
        println!("✓ Using browser for cookies: {}", browser);
    } else {
        println!("⚠️  Cookies not configured, download may not work");
    }

    // Add client settings
    // Using android client which does not require PO Token
    let player_client = "youtube:player_client=android";

    cmd.args(["--extractor-args", player_client, "--no-check-certificate", test_url]);

    println!("Running command: {:?}", cmd);
    let output = cmd
        .stdout(Stdio::inherit())
        .stderr(Stdio::piped())
        .output()
        .expect("Failed to run yt-dlp");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        println!("\n❌ ERROR during download:");
        println!("{}", stderr);

        // Detailed error analysis
        if stderr.contains("Please sign in") {
            println!("\n🔴 PROBLEM: Authentication required");
            println!("💡 SOLUTION:");
            println!("   1. Export cookies from your browser");
            println!("   2. Set: export YTDL_COOKIES_FILE=./youtube_cookies.txt");
            println!("   3. Re-run the test");
            println!("\n   Detailed instructions: MACOS_COOKIES_FIX.md");
        }

        if stderr.contains("PO Token") || stderr.contains("GVS PO Token") {
            println!("\n🔴 PROBLEM: PO Token required (new YouTube requirement)");
            println!("💡 SOLUTION:");
            println!("   1. Update yt-dlp: pip3 install -U yt-dlp");
            println!("   2. Make sure you are using cookies");
        }

        if stderr.contains("HTTP Error 403") || stderr.contains("bot detection") {
            println!("\n🔴 PROBLEM: YouTube blocked the request (bot detected)");
            println!("💡 SOLUTION:");
            println!("   1. Make sure to use cookies");
            println!("   2. Try a different player_client");
        }

        if stderr.contains("formats have been skipped") {
            println!("\n⚠️  WARNING: Some formats were skipped");
            println!("   This is normal, continuing with available formats");
        }

        panic!("Download failed");
    }

    // Give time for ffmpeg conversion to finish
    std::thread::sleep(Duration::from_secs(2));

    // Check that the file was created and is not empty
    assert!(output_file.exists(), "File was not created: {:?}", output_file);

    let metadata = fs::metadata(&output_file).expect("Failed to get file metadata");
    println!("✓ File created: {:?}", output_file);
    println!(
        "✓ File size: {} bytes ({:.2} MB)",
        metadata.len(),
        metadata.len() as f64 / 1024.0 / 1024.0
    );

    assert!(metadata.len() > 0, "File is empty");
    assert!(metadata.len() > 10000, "File is too small (possibly corrupted)");

    // Clean up
    cleanup_test_dir(&tmp_dir);
    println!("✓ Test completed successfully");
}

/// Test 6: Check error handling (invalid URL)
#[test]
#[ignore]
fn test_ytdlp_invalid_url() {
    if !command_exists("yt-dlp") {
        println!("⚠️  yt-dlp is not installed, skipping test");
        return;
    }

    println!("=== Invalid URL error handling test ===");

    let invalid_url = "https://www.youtube.com/watch?v=INVALID_VIDEO_ID_12345";

    let output = Command::new("yt-dlp")
        .args(["--get-title", "--no-playlist", invalid_url])
        .output()
        .expect("Failed to run yt-dlp");

    // Expect the command to exit with an error
    assert!(
        !output.status.success(),
        "Command should have exited with error for invalid URL"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("✓ Expected error received:");
    println!("{}", stderr);

    // Check that the error contains relevant information
    assert!(
        stderr.contains("ERROR") || stderr.contains("Video unavailable") || stderr.contains("not available"),
        "Error should contain information about video unavailability"
    );
}

/// Test 7: Check download with different quality settings
#[test]
#[ignore]
fn test_ytdlp_different_qualities() {
    if !command_exists("yt-dlp") || !command_exists("ffmpeg") {
        println!("⚠️  yt-dlp or ffmpeg is not installed, skipping test");
        return;
    }

    println!("=== Download with different quality settings test ===");

    let tmp_dir = get_test_downloads_dir();
    cleanup_test_dir(&tmp_dir);

    let test_url = "https://www.youtube.com/watch?v=jNQXAC9IVRw";
    let qualities = vec![("320k", "320k"), ("192k", "192k"), ("128k", "128k")];

    for (name, bitrate) in qualities {
        println!("\n--- Quality test: {} ---", name);
        let output_file = tmp_dir.join(format!("test_audio_{}.mp3", name));

        let mut cmd = Command::new("yt-dlp");
        cmd.args([
            "-o",
            output_file.to_str().expect("Invalid UTF-8 in output file path"),
            "--extract-audio",
            "--audio-format",
            "mp3",
            "--audio-quality",
            "0",
            "--no-playlist",
            "--postprocessor-args",
            &format!("-acodec libmp3lame -b:a {}", bitrate),
        ]);

        // Add cookies if available
        if let Some(cookies_file) = get_cookies_file() {
            cmd.args(["--cookies", &cookies_file]);
        } else if let Some(browser) = get_cookies_browser() {
            cmd.args(["--cookies-from-browser", &browser]);
        }

        cmd.arg(test_url);

        let output = cmd
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .expect("Failed to run yt-dlp");

        if output.status.success() {
            std::thread::sleep(Duration::from_secs(2));

            if output_file.exists() {
                let size = fs::metadata(&output_file)
                    .expect("Failed to read file metadata in test")
                    .len();
                println!("✓ Quality {}: {} bytes", name, size);
            } else {
                println!("⚠️  File not created for quality {}", name);
            }
        } else {
            println!("⚠️  Download failed for quality {}", name);
        }
    }

    cleanup_test_dir(&tmp_dir);
    println!("\n✓ Different quality test completed");
}

/// Helper function: Full system diagnostics
#[test]
fn test_full_diagnostics() {
    println!("\n╔════════════════════════════════════════════════════════════════╗");
    println!("║            FULL DOWNLOAD SYSTEM DIAGNOSTICS                   ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");

    // 1. Check tools
    println!("📦 1. INSTALLED TOOLS:");
    let tools = vec![
        ("yt-dlp", command_exists("yt-dlp")),
        ("ffmpeg", command_exists("ffmpeg")),
        ("ffprobe", command_exists("ffprobe")),
    ];

    for (tool, exists) in &tools {
        let status = if *exists { "✅ Installed" } else { "❌ NOT INSTALLED" };
        println!("   {} : {}", tool, status);
    }

    // 2. Versions
    println!("\n📋 2. VERSIONS:");
    if command_exists("yt-dlp") {
        if let Ok(output) = Command::new("yt-dlp").arg("--version").output() {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            println!("   yt-dlp: {}", version);
        }
    }

    if command_exists("ffmpeg") {
        if let Ok(output) = Command::new("ffmpeg").arg("-version").output() {
            let version_line = String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or("unknown")
                .to_string();
            println!("   ffmpeg: {}", version_line);
        }
    }

    // 3. Cookies configuration
    println!("\n🍪 3. COOKIES CONFIGURATION:");
    match (get_cookies_file(), get_cookies_browser()) {
        (Some(file), _) => {
            println!("   Type: File");
            println!("   Path: {}", file);
            if std::path::Path::new(&file).exists() {
                let size = fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
                println!("   Status: ✅ Exists ({} bytes)", size);
            } else {
                println!("   Status: ❌ FILE NOT FOUND");
            }
        }
        (None, Some(browser)) => {
            println!("   Type: Browser");
            println!("   Browser: {}", browser);
            println!("   Status: ⚠️  Requires Full Disk Access on macOS");
        }
        (None, None) => {
            println!("   Status: ❌ NOT CONFIGURED");
            println!("\n   📖 Setup instructions:");
            println!("      export YTDL_COOKIES_FILE=./youtube_cookies.txt");
            println!("      See MACOS_COOKIES_FIX.md for details");
        }
    }

    // 4. Environment variables
    println!("\n🔧 4. ENVIRONMENT VARIABLES:");
    let env_vars = vec!["YTDL_COOKIES_FILE", "YTDL_COOKIES_BROWSER", "YTDL_BIN"];

    for var in env_vars {
        match env::var(var) {
            Ok(value) => println!("   {}: {}", var, value),
            Err(_) => println!("   {}: (not set)", var),
        }
    }

    // 5. Overall assessment
    println!("\n📊 5. OVERALL ASSESSMENT:");
    let all_tools_ok = tools.iter().all(|(_, exists)| *exists);
    let cookies_ok = get_cookies_file().is_some() || get_cookies_browser().is_some();

    if all_tools_ok && cookies_ok {
        println!("   ✅ System is ready!");
    } else {
        println!("   ⚠️  Issues detected:");
        if !all_tools_ok {
            println!("      • Not all required tools are installed");
        }
        if !cookies_ok {
            println!("      • Cookies not configured (YouTube videos will not download)");
        }
    }

    println!("\n╔════════════════════════════════════════════════════════════════╗");
    println!("║                   DIAGNOSTICS COMPLETE                         ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");
}
