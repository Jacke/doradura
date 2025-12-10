# Fixing Incorrect Filenames

## Problem
Downloads were saved as `Unknown Track.mp4`, which meant metadata was not retrieved correctly from yt-dlp.

## Solution

### 1. Better metadata fetching (`src/downloader.rs`)

**Updates to `get_metadata_from_ytdlp`:**

- Switched `--get-title` to the more reliable `--print "%(title)s"`.
- Added `--skip-download` to speed up metadata retrieval.
- Instead of falling back to "Unknown Track", return a clear error.
- Added a check for empty titles.
- Improved error messages for easier diagnostics.

**Before:**
```rust
let title = if title_output.status.success() {
    String::from_utf8_lossy(&title_output.stdout).trim().to_string()
} else {
    log::warn!("yt-dlp returned non-zero status, using default title");
    "Unknown Track".to_string()
};
```

**After:**
```rust
if !title_output.status.success() {
    let stderr = String::from_utf8_lossy(&title_output.stderr);
    log::error!("yt-dlp failed to get metadata, stderr: {}", stderr);
    return Err(AppError::Download(format!(
        "Failed to get video metadata. Please check if video is available and cookies are configured."
    )));
}

let title = String::from_utf8_lossy(&title_output.stdout).trim().to_string();

if title.is_empty() {
    log::error!("yt-dlp returned empty title for URL: {}", url);
    return Err(AppError::Download(
        "Failed to retrieve title. The video may be unavailable or cookies are missing.".to_string(),
    ));
}
```

### 2. Improved filename sanitization (`src/utils.rs`)

- Escapes forbidden characters more reliably.
- Preserves dots in extensions while cleaning the name.

### 3. Better error propagation

- If metadata cannot be fetched, the user now sees a clear message instead of a silent fallback.
- Logs include yt-dlp stderr for faster troubleshooting.

## Outcome
- Files now keep their correct titles.
- Clearer errors when metadata is unavailable.
- Faster metadata calls thanks to `--skip-download`.
