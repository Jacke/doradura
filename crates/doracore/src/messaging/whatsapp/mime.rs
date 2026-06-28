//! MIME-type inference for media uploads — WhatsApp's media endpoint requires a
//! `type` field, and the Cloud API only accepts a known set per message kind.

use crate::messaging::types::MediaKind;

/// Best-effort MIME type for a local file about to be uploaded, chosen from its
/// extension and falling back to a sensible default for the [`MediaKind`].
pub fn guess_mime(kind: MediaKind, path: &str) -> &'static str {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    match ext.as_str() {
        // audio
        "mp3" => "audio/mpeg",
        "m4a" | "aac" => "audio/mp4",
        "ogg" | "opus" => "audio/ogg",
        "amr" => "audio/amr",
        // video
        "mp4" | "m4v" => "video/mp4",
        "3gp" | "3gpp" => "video/3gpp",
        // image
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        // document
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "zip" => "application/zip",
        // unknown → default by kind
        _ => default_mime(kind),
    }
}

/// Default MIME when the extension is unknown.
fn default_mime(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Audio => "audio/mpeg",
        MediaKind::Video | MediaKind::VideoNote | MediaKind::Animation => "video/mp4",
        MediaKind::Photo => "image/jpeg",
        MediaKind::Document => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_common_extensions() {
        assert_eq!(guess_mime(MediaKind::Audio, "/t/song.mp3"), "audio/mpeg");
        assert_eq!(guess_mime(MediaKind::Audio, "/t/song.m4a"), "audio/mp4");
        assert_eq!(guess_mime(MediaKind::Video, "/t/clip.mp4"), "video/mp4");
        assert_eq!(guess_mime(MediaKind::Photo, "/t/cover.JPG"), "image/jpeg");
        assert_eq!(guess_mime(MediaKind::Document, "/t/notes.pdf"), "application/pdf");
    }

    #[test]
    fn falls_back_by_kind_for_unknown_ext() {
        assert_eq!(guess_mime(MediaKind::Audio, "/t/weird.xyz"), "audio/mpeg");
        assert_eq!(guess_mime(MediaKind::Document, "/t/weird"), "application/octet-stream");
    }
}
