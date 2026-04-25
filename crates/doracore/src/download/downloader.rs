//! Core download utilities — filename generation, progress parsing, partial file cleanup.
//! No Telegram dependencies.

use crate::core::utils::sanitize_filename;
use crate::download::source::SourceProgress;
use fs_err as fs;
use std::path::Path;

/// Cleans up all partial/temporary files created by yt-dlp for a download path.
///
/// yt-dlp creates various intermediate files during download:
/// - `.part` — partial download
/// - `.ytdl` — download state
/// - `.temp.{ext}` — temporary merge files
/// - `.f{N}.{ext}` — format-specific fragments
/// - `.info.json` — metadata cache
///
/// This function removes all of them to prevent disk space leaks.
pub fn cleanup_partial_download(base_path: &str) {
    let base = Path::new(base_path);
    let parent = base.parent().unwrap_or(Path::new("."));
    let filename = base.file_name().and_then(|n| n.to_str()).unwrap_or("");

    // Remove exact known patterns
    let patterns = [".part", ".ytdl", ".temp.mp4", ".temp.webm", ".info.json"];
    for pattern in patterns {
        let path = format!("{}{}", base_path, pattern);
        if let Err(e) = fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::debug!("Failed to remove {}: {}", path, e);
            }
        }
    }

    // Remove fragment files (.f{N}.{ext}) using directory scan.
    // These are created when yt-dlp downloads separate audio/video streams.
    if let Ok(entries) = fs::read_dir(parent) {
        let base_name = filename
            .trim_end_matches(".mp4")
            .trim_end_matches(".mp3")
            .trim_end_matches(".webm");
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                // Match patterns like: basename.f123.mp4, basename.f456.webm, etc.
                if name.starts_with(base_name)
                    && (name.contains(".f") || name.ends_with(".part") || name.ends_with(".ytdl"))
                {
                    let path = entry.path();
                    if let Err(e) = fs::remove_file(&path) {
                        if e.kind() != std::io::ErrorKind::NotFound {
                            log::debug!("Failed to remove fragment {}: {}", path.display(), e);
                        }
                    }
                }
            }
        }
    }

    log::debug!("Cleaned up partial files for: {}", base_path);
}

/// Parses a yt-dlp download progress line into a [`SourceProgress`].
///
/// Recognises lines of the form produced by yt-dlp's text output:
/// ```text
/// [download]  45.2% of 10.00MiB at 500.00KiB/s ETA 00:10
/// ```
///
/// Returns `None` if the line does not contain parseable progress data.
pub fn parse_progress(line: &str) -> Option<SourceProgress> {
    // Require the [download] prefix and a percent value
    if !line.contains("[download]") || !line.contains('%') {
        if line.contains("[download]") {
            log::trace!("Download line without percent: {}", line);
        }
        return None;
    }

    let mut percent: Option<u8> = None;
    let mut speed_bytes_sec: Option<f64> = None;
    let mut eta_seconds: Option<u64> = None;
    let mut downloaded_bytes: Option<u64> = None;
    let mut total_bytes: Option<u64> = None;

    // Parse without Vec allocation — use a peekable iterator over whitespace-split parts
    let mut parts = line.split_whitespace().peekable();
    while let Some(part) = parts.next() {
        // Parse percent: "45.2%"
        if part.ends_with('%') {
            if let Ok(p) = part.trim_end_matches('%').parse::<f32>() {
                percent = Some(p.clamp(0.0, 100.0) as u8);
            }
        }

        // Parse total size: "of 10.00MiB"
        if part == "of" {
            if let Some(&next) = parts.peek() {
                if let Some(bytes) = parse_size(next) {
                    total_bytes = Some(bytes);
                }
            }
        }

        // Parse speed: "at 500.00KiB/s"
        if part == "at" {
            if let Some(&next) = parts.peek() {
                if let Some(bytes_per_sec) = parse_size(next) {
                    speed_bytes_sec = Some(bytes_per_sec as f64);
                }
            }
        }

        // Parse ETA: "ETA 00:10"
        if part == "ETA" {
            if let Some(&next) = parts.peek() {
                if let Some(eta) = parse_eta(next) {
                    eta_seconds = Some(eta);
                }
            }
        }
    }

    let p = percent?;

    // Derive downloaded bytes from percent + total
    if let Some(total) = total_bytes {
        downloaded_bytes = Some((total as f64 * (p as f64 / 100.0)) as u64);
    }

    log::debug!(
        "Progress parsed: {}% (speed: {:?} B/s, eta: {:?}s)",
        p,
        speed_bytes_sec,
        eta_seconds,
    );

    Some(SourceProgress {
        percent: p,
        speed_bytes_sec,
        eta_seconds,
        downloaded_bytes,
        total_bytes,
        ..Default::default()
    })
}

/// Parse a yt-dlp / ffmpeg merge-step progress line into a
/// [`SourceProgress`] with [`ProgressPhase::Merging`].
///
/// During the post-download muxing phase yt-dlp shells out to ffmpeg and
/// pipes its stderr through. ffmpeg emits progress as
/// `frame= 1234 fps=180 q=29.0 size= 256kB time=00:00:42.34 bitrate=…`,
/// where `time=` is the position into the input that has been consumed.
/// We can't compute a percent here (the parser doesn't know the total
/// duration), so we hand the consumer the raw seconds and let it divide
/// by the known media duration.
///
/// Also matches the `[Merger]` notification line so the consumer can flip
/// the UI to "merging" state immediately, even before the first ffmpeg
/// progress tick lands.
pub fn parse_merge_progress(line: &str) -> Option<SourceProgress> {
    if line.contains("[Merger]") {
        return Some(SourceProgress {
            phase: crate::download::source::ProgressPhase::Merging,
            merge_position_secs: Some(0.0),
            ..Default::default()
        });
    }
    if !line.contains("time=") || !line.contains("bitrate=") {
        return None;
    }
    let time_token = line.split_whitespace().find_map(|tok| tok.strip_prefix("time="))?;
    let secs = parse_ffmpeg_time(time_token)?;
    Some(SourceProgress {
        phase: crate::download::source::ProgressPhase::Merging,
        merge_position_secs: Some(secs),
        ..Default::default()
    })
}

/// Parses an ffmpeg `time=HH:MM:SS.ms` token into seconds.
fn parse_ffmpeg_time(token: &str) -> Option<f32> {
    if token == "N/A" {
        return None;
    }
    let mut parts = token.split(':');
    let h: f32 = parts.next()?.parse().ok()?;
    let m: f32 = parts.next()?.parse().ok()?;
    let s: f32 = parts.next()?.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + s)
}

/// Parses a human-readable size string like `"10.00MiB"` or `"500.00KiB/s"` into bytes.
fn parse_size(size_str: &str) -> Option<u64> {
    let s = size_str.trim_end_matches("/s");
    if let Some(n) = s.strip_suffix("GiB") {
        return n.parse::<f64>().ok().map(|g| (g * 1024.0 * 1024.0 * 1024.0) as u64);
    }
    if let Some(n) = s.strip_suffix("MiB") {
        return n.parse::<f64>().ok().map(|m| (m * 1024.0 * 1024.0) as u64);
    }
    if let Some(n) = s.strip_suffix("KiB") {
        return n.parse::<f64>().ok().map(|k| (k * 1024.0) as u64);
    }
    None
}

/// Parses an ETA string like `"00:10"` or `"1:23"` into total seconds.
fn parse_eta(eta_str: &str) -> Option<u64> {
    let (minutes_str, seconds_str) = eta_str.split_once(':')?;
    let minutes: u64 = minutes_str.parse().ok()?;
    let seconds: u64 = seconds_str.parse().ok()?;
    Some(minutes * 60 + seconds)
}

/// Generates a sanitized filename with `.mp3` extension from title and artist.
///
/// This is a convenience wrapper around [`generate_file_name_with_ext`].
pub fn generate_file_name(title: &str, artist: &str) -> String {
    generate_file_name_with_ext(title, artist, "mp3")
}

/// Generates a sanitized filename from title, artist, and file extension.
///
/// Rules:
/// - Both empty → `"Unknown.{ext}"`
/// - Artist empty → `"{title}.{ext}"`
/// - Title empty → `"{artist}.{ext}"`
/// - Both present → `"{artist} - {title}.{ext}"`
///
/// The result is passed through [`sanitize_filename`] to make it filesystem-safe.
pub fn generate_file_name_with_ext(title: &str, artist: &str, extension: &str) -> String {
    let title_trimmed = title.trim();
    let artist_trimmed = artist.trim();

    log::debug!(
        "Generating filename: title='{}' (len={}), artist='{}' (len={}), ext='{}'",
        title,
        title.len(),
        artist,
        artist.len(),
        extension,
    );

    let filename = if artist_trimmed.is_empty() && title_trimmed.is_empty() {
        log::warn!("Both title and artist are empty, using 'Unknown.{}'", extension);
        format!("Unknown.{}", extension)
    } else if artist_trimmed.is_empty() {
        log::debug!("Using title only: '{}.{}'", title_trimmed, extension);
        format!("{}.{}", title_trimmed, extension)
    } else if title_trimmed.is_empty() {
        log::debug!("Using artist only: '{}.{}'", artist_trimmed, extension);
        format!("{}.{}", artist_trimmed, extension)
    } else {
        log::debug!("Using both: '{} - {}.{}'", artist_trimmed, title_trimmed, extension,);
        format!("{} - {}.{}", artist_trimmed, title_trimmed, extension)
    };

    sanitize_filename(&filename)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_file_name_both_present() {
        let name = generate_file_name_with_ext("Song", "Artist", "mp3");
        assert_eq!(name, "Artist_-_Song.mp3");
    }

    #[test]
    fn test_generate_file_name_no_artist() {
        let name = generate_file_name_with_ext("Song", "", "mp3");
        assert_eq!(name, "Song.mp3");
    }

    #[test]
    fn test_generate_file_name_no_title() {
        let name = generate_file_name_with_ext("", "Artist", "mp3");
        assert_eq!(name, "Artist.mp3");
    }

    #[test]
    fn test_generate_file_name_both_empty() {
        let name = generate_file_name_with_ext("", "", "mp3");
        assert_eq!(name, "Unknown.mp3");
    }

    #[test]
    fn test_generate_file_name_default_ext() {
        let name = generate_file_name("Song", "Artist");
        assert!(name.ends_with(".mp3"));
    }

    #[test]
    fn test_parse_progress_basic() {
        let line = "[download]  45.2% of 10.00MiB at 500.00KiB/s ETA 00:10";
        let progress = parse_progress(line).expect("should parse");
        assert_eq!(progress.percent, 45);
        assert!(progress.speed_bytes_sec.is_some());
        assert_eq!(progress.eta_seconds, Some(10));
        assert!(progress.total_bytes.is_some());
    }

    #[test]
    fn test_parse_progress_no_download_tag() {
        let line = "some random line 50%";
        assert!(parse_progress(line).is_none());
    }

    #[test]
    fn test_parse_progress_no_percent() {
        let line = "[download] Destination: file.mp4";
        assert!(parse_progress(line).is_none());
    }

    #[test]
    fn test_cleanup_partial_download_noop_on_missing() {
        // Should not panic when files do not exist
        cleanup_partial_download("/tmp/__nonexistent_doradura_test_file__.mp3");
    }
}
