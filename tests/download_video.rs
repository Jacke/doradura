use std::process::Command;
use std::time::Duration;
use std::{env, fs};

fn which(bin: &str) -> bool {
    Command::new("bash")
        .arg("-lc")
        .arg(format!("command -v {} >/dev/null 2>&1", bin))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
#[ignore]
fn download_youtube_video_with_ytdlp() {
    // Require yt-dlp and ffmpeg in PATH; skip if missing
    if !which("yt-dlp") {
        eprintln!("yt-dlp not found in PATH; skipping test");
        return;
    }
    if !which("ffmpeg") {
        eprintln!("ffmpeg not found in PATH; skipping test");
        return;
    }

    let url = "https://youtu.be/cwHZg9PQtV0";

    // Prepare output directory and template
    let tmp_dir = env::temp_dir().join("doradura_test_downloads");
    let _ = fs::create_dir_all(&tmp_dir);
    let out_prefix = tmp_dir.join("test_video");
    let out_template = format!("{}.% (ext)s", out_prefix.display()).replace(" ", "");

    // Run yt-dlp to fetch best available format
    let status = Command::new("yt-dlp")
        // prefer small progressive/audio formats to avoid HLS trimming issues
        .args(["-f", "140/bestaudio[ext=m4a]/bestaudio/best"]) // prefer m4a
        .args(["--format-sort", "acodec:m4a,ext:mp4,res,br"])
        .args(["--max-filesize", "50M"]) // cap size
        .args(["--no-playlist"]) // safety
        .args(["-o", &out_template])
        .arg(url)
        .status()
        .expect("failed to spawn yt-dlp");
    if !status.success() {
        eprintln!("skipping: yt-dlp exited with status {:?}", status.code());
        return; // treat as skipped in constrained envs (403, format not available)
    }

    // Give filesystem a moment (especially on CI) and then verify file exists and non-empty
    std::thread::sleep(Duration::from_millis(200));

    // Find the produced file (extension may vary)
    let mut found = false;
    if let Ok(entries) = fs::read_dir(&tmp_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("test_video.") {
                    let md = fs::metadata(&path).expect("metadata failed");
                    assert!(md.len() > 0, "downloaded file is empty: {}", path.display());
                    found = true;
                    // Cleanup after ourselves
                    let _ = fs::remove_file(&path);
                }
            }
        }
    }
    assert!(found, "no downloaded file matching prefix found in {}", tmp_dir.display());
}


