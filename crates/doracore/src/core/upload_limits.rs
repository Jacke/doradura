//! Single source of truth for Telegram upload size limits.
//!
//! Telegram exposes different per-method size caps:
//!
//! | Method               | Cloud Bot API | Local Bot API |
//! |----------------------|---------------|---------------|
//! | sendVideo / Document / Audio / Animation | 50 MB         | 2000 MB       |
//! | sendVideoNote (circle) | 12 MiB + ≤60s  | 12 MiB         |
//! | sendVoice            | 50 MB         | 50 MB         |
//! | sendPhoto            | 10 MB         | 10 MB         |
//!
//! [`UploadLimits`] reads `BOT_API_URL` to pick the right column. The video /
//! audio thresholds delegate to the existing
//! [`crate::core::config::validation::max_video_size_bytes`] /
//! [`max_audio_size_bytes`] getters so behaviour stays identical to the
//! legacy code path; the rest of the kinds are spec-derived constants.
//!
//! Use [`UploadLimits::default()`] or [`UploadLimits::from_env`] to obtain a
//! concrete instance, then call [`UploadLimits::check`] to validate before
//! upload. A failed check returns [`UploadTooLarge`] which carries the kind,
//! actual size, and cap so callers can localize the error message.

use crate::core::config;

/// Telegram method category — drives which size cap applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadKind {
    /// `sendVideo`
    Video,
    /// `sendAudio`
    Audio,
    /// `sendDocument`
    Document,
    /// `sendPhoto` (10 MB hard cap on both APIs)
    Photo,
    /// `sendVideoNote` — 12 MiB cap, ≤60 s, square aspect
    VideoNote,
    /// `sendVoice` — 50 MB cap (does not benefit from local API)
    Voice,
    /// `sendAnimation` (GIF/MP4 silent loop)
    Animation,
}

impl UploadKind {
    /// Stable string identifier for logging / metrics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Document => "document",
            Self::Photo => "photo",
            Self::VideoNote => "video_note",
            Self::Voice => "voice",
            Self::Animation => "animation",
        }
    }
}

/// Resolved size caps (in bytes) for the current Bot API mode.
#[derive(Debug, Clone, Copy)]
pub struct UploadLimits {
    pub video: u64,
    pub audio: u64,
    pub document: u64,
    pub photo: u64,
    pub video_note: u64,
    pub voice: u64,
    pub animation: u64,
}

const PHOTO_CAP: u64 = 10 * 1024 * 1024; // 10 MB — same on both APIs
const VIDEO_NOTE_CAP: u64 = 12 * 1024 * 1024; // 12 MiB
const VOICE_CAP: u64 = 50 * 1024 * 1024; // 50 MB
const CLOUD_API_FILE_CAP: u64 = 50 * 1024 * 1024; // 50 MB cloud cap

impl UploadLimits {
    /// Pick caps based on the configured Bot API endpoint. Local Bot API
    /// (any `BOT_API_URL` not pointing at `api.telegram.org`) unlocks the
    /// large-file caps for video/audio/document/animation.
    pub fn from_env() -> Self {
        let video = config::validation::max_video_size_bytes();
        let audio = config::validation::max_audio_size_bytes();
        // Document and animation share the video cap on Telegram (both go
        // through the same large-file route on local Bot API).
        let document = video;
        let animation = video;
        Self {
            video,
            audio,
            document,
            animation,
            photo: PHOTO_CAP,
            video_note: VIDEO_NOTE_CAP,
            voice: VOICE_CAP,
        }
    }

    /// Build with a uniform cap applied to large-file kinds (test helper).
    #[doc(hidden)]
    pub fn with_uniform_cap(cap: u64) -> Self {
        Self {
            video: cap,
            audio: cap,
            document: cap,
            animation: cap,
            photo: PHOTO_CAP,
            video_note: VIDEO_NOTE_CAP,
            voice: VOICE_CAP,
        }
    }

    /// Pre-Local-API defaults (all 50 MB except Photo / VideoNote).
    #[doc(hidden)]
    pub fn cloud_only() -> Self {
        Self::with_uniform_cap(CLOUD_API_FILE_CAP)
    }

    /// Cap (in bytes) for the given kind.
    pub fn cap(&self, kind: UploadKind) -> u64 {
        match kind {
            UploadKind::Video => self.video,
            UploadKind::Audio => self.audio,
            UploadKind::Document => self.document,
            UploadKind::Photo => self.photo,
            UploadKind::VideoNote => self.video_note,
            UploadKind::Voice => self.voice,
            UploadKind::Animation => self.animation,
        }
    }

    /// Validate that `size_bytes` fits within the cap for `kind`. Returns
    /// `Ok(())` on success, [`UploadTooLarge`] otherwise.
    pub fn check(&self, kind: UploadKind, size_bytes: u64) -> Result<(), UploadTooLarge> {
        let cap = self.cap(kind);
        if size_bytes <= cap {
            Ok(())
        } else {
            Err(UploadTooLarge {
                kind,
                size_bytes,
                cap_bytes: cap,
            })
        }
    }
}

impl Default for UploadLimits {
    fn default() -> Self {
        Self::from_env()
    }
}

/// Validation failure: file exceeds the per-method Telegram cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadTooLarge {
    pub kind: UploadKind,
    pub size_bytes: u64,
    pub cap_bytes: u64,
}

impl UploadTooLarge {
    pub fn size_mb(&self) -> f64 {
        self.size_bytes as f64 / (1024.0 * 1024.0)
    }
    pub fn cap_mb(&self) -> f64 {
        self.cap_bytes as f64 / (1024.0 * 1024.0)
    }
}

impl std::fmt::Display for UploadTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} too large: {:.2} MB exceeds Telegram cap of {:.2} MB",
            self.kind.as_str(),
            self.size_mb(),
            self.cap_mb(),
        )
    }
}

impl std::error::Error for UploadTooLarge {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_only_uniform_cap_is_50mb() {
        let limits = UploadLimits::cloud_only();
        assert_eq!(limits.cap(UploadKind::Video), CLOUD_API_FILE_CAP);
        assert_eq!(limits.cap(UploadKind::Audio), CLOUD_API_FILE_CAP);
        assert_eq!(limits.cap(UploadKind::Document), CLOUD_API_FILE_CAP);
        assert_eq!(limits.cap(UploadKind::Animation), CLOUD_API_FILE_CAP);
    }

    #[test]
    fn photo_video_note_voice_are_spec_constants() {
        let limits = UploadLimits::with_uniform_cap(2 * 1024 * 1024 * 1024);
        // These should NOT bump up with the local API mode.
        assert_eq!(limits.cap(UploadKind::Photo), PHOTO_CAP);
        assert_eq!(limits.cap(UploadKind::VideoNote), VIDEO_NOTE_CAP);
        assert_eq!(limits.cap(UploadKind::Voice), VOICE_CAP);
    }

    #[test]
    fn check_passes_at_or_below_cap() {
        let limits = UploadLimits::cloud_only();
        assert!(limits.check(UploadKind::Video, 0).is_ok());
        assert!(limits.check(UploadKind::Video, CLOUD_API_FILE_CAP).is_ok());
        assert!(limits.check(UploadKind::Photo, PHOTO_CAP).is_ok());
    }

    #[test]
    fn check_fails_above_cap_with_kind_and_sizes() {
        let limits = UploadLimits::cloud_only();
        let err = limits.check(UploadKind::Video, CLOUD_API_FILE_CAP + 1).unwrap_err();
        assert_eq!(err.kind, UploadKind::Video);
        assert_eq!(err.size_bytes, CLOUD_API_FILE_CAP + 1);
        assert_eq!(err.cap_bytes, CLOUD_API_FILE_CAP);
    }

    #[test]
    fn upload_too_large_display_includes_kind_and_mb() {
        let err = UploadTooLarge {
            kind: UploadKind::Audio,
            size_bytes: 100 * 1024 * 1024,
            cap_bytes: 50 * 1024 * 1024,
        };
        let s = format!("{}", err);
        assert!(s.contains("audio"));
        assert!(s.contains("100.00 MB"));
        assert!(s.contains("50.00 MB"));
    }

    #[test]
    fn video_note_cap_is_12_mib_exactly() {
        // 12 MiB = 12 × 1024 × 1024 = 12_582_912 bytes per Telegram spec.
        assert_eq!(VIDEO_NOTE_CAP, 12_582_912);
    }

    #[test]
    fn photo_cap_is_10_mb_exactly() {
        assert_eq!(PHOTO_CAP, 10 * 1024 * 1024);
    }

    #[test]
    fn voice_cap_does_not_benefit_from_local_api() {
        // Voice notes are 50 MB even on local Bot API.
        let local = UploadLimits::with_uniform_cap(2 * 1024 * 1024 * 1024);
        assert_eq!(local.cap(UploadKind::Voice), 50 * 1024 * 1024);
    }

    #[test]
    fn kind_as_str_is_stable_for_metrics() {
        assert_eq!(UploadKind::Video.as_str(), "video");
        assert_eq!(UploadKind::VideoNote.as_str(), "video_note");
        assert_eq!(UploadKind::Voice.as_str(), "voice");
    }

    #[test]
    fn check_zero_bytes_is_always_ok() {
        let limits = UploadLimits::cloud_only();
        for kind in [
            UploadKind::Video,
            UploadKind::Audio,
            UploadKind::Document,
            UploadKind::Photo,
            UploadKind::VideoNote,
            UploadKind::Voice,
            UploadKind::Animation,
        ] {
            assert!(limits.check(kind, 0).is_ok(), "kind {:?} rejected zero bytes", kind);
        }
    }
}
