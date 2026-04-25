//! Typed builder for the `-x264-params` argument string.
//!
//! `-x264-params` is a colon-separated list of `key=value` pairs read by
//! libx264 inside ffmpeg. The syntax is fragile in three ways the FFmpeg
//! CLI doesn't help with:
//!
//! 1. Some keys take a tuple value (e.g. `psy-rd=<rd>:<trellis>`,
//!    `deblock=<alpha>:<beta>`). The inner separator clashes with the outer
//!    `:` separator, so libx264 also accepts `,` for the inner — but you
//!    need to remember which.
//! 2. Boolean toggles must be `key=0` / `key=1`. The CLI flag `--no-foo`
//!    form does **not** work inside `-x264-params`; libx264 silently rejects
//!    the whole string with `Error setting option x264-params`.
//! 3. There is no key validation: a typo in the key name returns the same
//!    generic error, no diagnostics, no list of accepted keys.
//!
//! This module turns the string into a typed struct so all three failure
//! modes are caught at compile time: every key is a Rust field, every
//! value has a type, tuples are tuples, booleans render as `0`/`1`. The
//! `Display` impl is the only place that knows the wire syntax, and it's
//! covered by snapshot tests against the exact strings live ffmpeg accepts.

use std::fmt;

/// x264 motion-estimation algorithm. Sorted from fast/cheap to slow/dense.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionSearch {
    /// Diamond search — fastest, lowest quality. Used by `superfast`/`veryfast`.
    Dia,
    /// Hex (default for `medium`).
    Hex,
    /// Uneven multi-hexagon — strong quality/speed balance, default for
    /// `slow`/`slower`/`veryslow`.
    Umh,
    /// Exhaustive — slow.
    Esa,
    /// Transformed exhaustive — slowest, marginal gain over `esa` for video
    /// notes at 640²; rarely worth it.
    Tesa,
}

impl MotionSearch {
    fn as_str(self) -> &'static str {
        match self {
            Self::Dia => "dia",
            Self::Hex => "hex",
            Self::Umh => "umh",
            Self::Esa => "esa",
            Self::Tesa => "tesa",
        }
    }
}

/// Typed wrapper over the `-x264-params` value string.
///
/// Default-constructed = empty (no overrides on top of the active preset).
/// Build by chaining setter methods. Render with [`X264Params::to_arg_string`].
#[derive(Debug, Default, Clone)]
pub struct X264Params {
    /// Adaptive quantization mode. `3` biases bits into dark/flat blocks
    /// — preferred for live action with dark scenes.
    pub aq_mode: Option<u8>,

    /// AQ strength. Values >1.0 push harder into dark/flat areas; >1.3
    /// starts softening edges on live action.
    pub aq_strength: Option<f32>,

    /// `(rd, trellis)` — psychovisual rate-distortion. Live action ~1.0
    /// for the RD component; trellis-PSY 0.15-0.20 keeps fine detail.
    pub psy_rd: Option<(f32, f32)>,

    /// `(alpha, beta)` — deblocking. Negative values resist x264's default
    /// smoothing; useful for retaining detail in dark frames.
    pub deblock: Option<(i8, i8)>,

    /// Frames the rate-controller looks ahead. Higher = better bit
    /// distribution under a tight bitrate cap.
    pub rc_lookahead: Option<u32>,

    /// Disable the "fast P-skip" quality shortcut. Set to `false` for
    /// max quality; ignore (`None`) keeps preset's default.
    pub fast_pskip: Option<bool>,

    /// Override the preset's motion search algorithm.
    pub me: Option<MotionSearch>,

    /// Subpel motion-estimation refinement (1-11). Higher = slower, sharper.
    pub subme: Option<u8>,

    /// Reference frames count. Bumping this past 5 may push the stream
    /// out of H.264 level 4.0 — guard with care.
    pub refs: Option<u8>,

    /// B-frames count.
    pub bframes: Option<u8>,
}

impl X264Params {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn aq_mode(mut self, v: u8) -> Self {
        self.aq_mode = Some(v);
        self
    }

    pub fn aq_strength(mut self, v: f32) -> Self {
        self.aq_strength = Some(v);
        self
    }

    /// `psy-rd=<rd>,<trellis>`. The two-arg form is enforced so callers
    /// can't accidentally drop the trellis component.
    pub fn psy_rd(mut self, rd: f32, trellis: f32) -> Self {
        self.psy_rd = Some((rd, trellis));
        self
    }

    pub fn deblock(mut self, alpha: i8, beta: i8) -> Self {
        self.deblock = Some((alpha, beta));
        self
    }

    pub fn rc_lookahead(mut self, frames: u32) -> Self {
        self.rc_lookahead = Some(frames);
        self
    }

    pub fn fast_pskip(mut self, enabled: bool) -> Self {
        self.fast_pskip = Some(enabled);
        self
    }

    pub fn me(mut self, m: MotionSearch) -> Self {
        self.me = Some(m);
        self
    }

    pub fn subme(mut self, v: u8) -> Self {
        self.subme = Some(v);
        self
    }

    pub fn refs(mut self, v: u8) -> Self {
        self.refs = Some(v);
        self
    }

    pub fn bframes(mut self, v: u8) -> Self {
        self.bframes = Some(v);
        self
    }

    /// Render to the colon-separated `key=value` form expected by libx264.
    /// Returns an empty string when no fields are set.
    pub fn to_arg_string(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for X264Params {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<String> = Vec::new();
        if let Some(v) = self.aq_mode {
            parts.push(format!("aq-mode={v}"));
        }
        if let Some(v) = self.aq_strength {
            parts.push(format!("aq-strength={v}"));
        }
        if let Some((rd, t)) = self.psy_rd {
            // Inside `-x264-params`, the inner separator must be `,` —
            // `:` collides with the outer key-pair separator.
            parts.push(format!("psy-rd={rd},{t}"));
        }
        if let Some((a, b)) = self.deblock {
            parts.push(format!("deblock={a},{b}"));
        }
        if let Some(v) = self.rc_lookahead {
            parts.push(format!("rc-lookahead={v}"));
        }
        if let Some(b) = self.fast_pskip {
            // libx264 inside `-x264-params` only accepts `key=0/1`; the
            // CLI toggle form `no-fast-pskip` is rejected outright.
            parts.push(format!("fast-pskip={}", b as u8));
        }
        if let Some(m) = self.me {
            parts.push(format!("me={}", m.as_str()));
        }
        if let Some(v) = self.subme {
            parts.push(format!("subme={v}"));
        }
        if let Some(v) = self.refs {
            parts.push(format!("ref={v}"));
        }
        if let Some(v) = self.bframes {
            parts.push(format!("bframes={v}"));
        }
        write!(f, "{}", parts.join(":"))
    }
}

/// Canonical params used for video-note encoding. Tuned for live-action
/// content with dark scenes at Telegram's 12 MB / 60 s budget. Combined
/// with `-preset veryslow -tune film -crf 16 -maxrate 1500k`.
pub fn video_note_dark_scene() -> X264Params {
    X264Params::new()
        .aq_mode(3)
        .aq_strength(1.2)
        .psy_rd(1.0, 0.20)
        .deblock(-2, -1)
        .rc_lookahead(60)
        .fast_pskip(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Snapshot test: the rendered string must match the exact form
    /// libx264 accepts in production. Update only if a real ffmpeg run
    /// confirms the new form is valid.
    #[test]
    fn video_note_dark_scene_matches_known_good_string() {
        let s = video_note_dark_scene().to_arg_string();
        assert_eq!(
            s, "aq-mode=3:aq-strength=1.2:psy-rd=1,0.2:deblock=-2,-1:rc-lookahead=60:fast-pskip=0",
            "wire format changed — verify the new string with `ffmpeg -x264-params <s>` before updating this assertion"
        );
    }

    #[test]
    fn empty_params_render_to_empty_string() {
        assert_eq!(X264Params::new().to_arg_string(), "");
    }

    #[test]
    fn boolean_toggles_render_as_zero_or_one() {
        // libx264 inside -x264-params only accepts numeric toggles, never
        // `no-fast-pskip` / `fast-pskip` keyword forms.
        assert_eq!(X264Params::new().fast_pskip(false).to_arg_string(), "fast-pskip=0");
        assert_eq!(X264Params::new().fast_pskip(true).to_arg_string(), "fast-pskip=1");
    }

    #[test]
    fn psy_rd_and_deblock_use_comma_inner_separator() {
        // `:` would collide with the outer key-pair separator and break
        // the whole string. This was the v0.43.1 bug we are guarding against.
        let s = X264Params::new().psy_rd(1.0, 0.2).deblock(-2, -1).to_arg_string();
        assert!(s.contains("psy-rd=1,0.2"), "psy-rd inner separator must be `,`: {s}");
        assert!(s.contains("deblock=-2,-1"), "deblock inner separator must be `,`: {s}");
        assert!(!s.contains(":0.2"), "no inner colons allowed: {s}");
    }

    #[test]
    fn motion_search_renders_lowercase() {
        for (m, expect) in [
            (MotionSearch::Dia, "me=dia"),
            (MotionSearch::Hex, "me=hex"),
            (MotionSearch::Umh, "me=umh"),
            (MotionSearch::Esa, "me=esa"),
            (MotionSearch::Tesa, "me=tesa"),
        ] {
            assert_eq!(X264Params::new().me(m).to_arg_string(), expect);
        }
    }
}
