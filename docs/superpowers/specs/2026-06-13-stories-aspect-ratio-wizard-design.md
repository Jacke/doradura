# Stories: Aspect-Ratio Config + Setup Wizard — Design Spec

**Date:** 2026-06-13
**Status:** Approved (design)
**Builds on:** `crates/dorabot/src/telegram/downloads/stories.rs` (Instagram Stories: 9:16 reframe + segment, config card with reframe/seg/quality/delivery)

## Context

Stories currently hardcodes a 9:16 / 1080×1920 canvas (`STORY_W`/`STORY_H`). The
user wants: (1) choose the **aspect ratio** (incl. center-crop "show only the
centre, cut the rest"), and (2) a **setup wizard** that previews all aspect
ratios by generating samples and sending them, so the choice is visual. Plus
encode **optimization** (the ffmpeg pass is the bottleneck; on a shared box
unbounded sample encodes are a runaway risk — cf. the 2026-03-09 incident).

Decisions locked during brainstorming:
- **AR set:** 9:16 (1080×1920), 1:1 (1080×1080), 4:5 (1080×1350), 16:9
  (1920×1080), Original (source AR, no reframe).
- **Fit mode:** keep existing Crop (center) / Blur (blurred fill). Crop anchor =
  center (v1).
- **Wizard preview = hybrid:** Step 1 a single contact-sheet **image** (one
  still frame cropped to each AR, composed side-by-side) — one cheap op, no video
  encodes. Step 2 on AR pick → **one** short low-res sample **clip**.
- **Wizard trigger:** auto on first Stories use + an always-present "👁 Показать
  все AR" button on the config card. Chosen AR saved as the user's default.

## Goals / Non-goals

**Goals**
- `AspectRatio` is a first-class Story setting; the ffmpeg reframe generalizes
  from fixed 9:16 to any chosen W×H.
- Center-crop to the chosen AR ("only the centre").
- Wizard: contact-sheet image → pick → one sample clip → save default.
- Bounded encodes (no runaway): sheet = 1 still op; sample = 1 short low-res clip.
- Optimization: `Original` + no reframe → stream-copy (`-c copy`), no re-encode.

**Non-goals (deferred)**
- Crop anchor other than center (top/face-track) — v2.
- Custom/arbitrary AR input — only the 5 presets.
- Per-AR multi-export in one run (export the same clip to several ARs at once).
- Hardware-accel encode (not available on the Railway box).

## Architecture — two phases, one module group

`stories.rs` stays the core; a new `stories/` submodule split keeps files focused:
- `downloads/stories.rs` — settings, config card, render+segment, send (existing,
  extended).
- `downloads/stories/aspect.rs` (new) — `AspectRatio` enum + `dims()` + filter
  builder (pure, unit-tested).
- `downloads/stories/wizard.rs` (new) — contact-sheet + sample-clip + wizard
  callbacks.

(If a flat layout is simpler given the existing single-file module, keep `aspect`
+ `wizard` as in-file modules; decide at implementation time, prefer small files.)

### Phase A — Aspect-ratio config

```rust
/// Target aspect ratio for the reframed Story. Dims are the 1080-base canvas;
/// `Original` keeps the source frame (no reframe → enables stream-copy).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AspectRatio { R9x16, R1x1, R4x5, R16x9, Original }

impl AspectRatio {
    /// Canvas dimensions, or `None` for `Original` (use source dims, no reframe).
    pub fn dims(self) -> Option<(u32, u32)> {
        match self {
            AspectRatio::R9x16 => Some((1080, 1920)),
            AspectRatio::R1x1  => Some((1080, 1080)),
            AspectRatio::R4x5  => Some((1080, 1350)),
            AspectRatio::R16x9 => Some((1920, 1080)),
            AspectRatio::Original => None,
        }
    }
    /// One-char token piece for callback encoding: t/q/p/w/o.
    pub fn token(self) -> char { /* R9x16='t', R1x1='q', R4x5='p', R16x9='w', Original='o' */ }
    pub fn from_token(c: char) -> Option<Self> { /* inverse */ }
    pub fn label(self) -> &'static str { /* "9:16" … "Original" */ }
}
```

- **Filter generalization:** the existing blur/crop filter builders take `(w,h)`
  instead of the `STORY_W`/`STORY_H` constants. Crop (center) =
  `scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h}`. Blur = fit +
  blurred {w}×{h} bg. `Original` → no scale/crop filter at all.
- **`StorySettings`** gains `aspect: AspectRatio`; token extended
  `<mode><seg><quality><delivery><ar>` (e.g. `b60svt`). `parse` already scans
  positionally (delivery work) → append AR scan after delivery; **missing AR char
  → default 9:16** (legacy tokens keep working). `encode` appends `aspect.token()`.
- **Config card:** add an AR row (5 buttons) above reframe; show current AR in the
  title. `with_aspect(ar)`.
- **Per-user default AR:** persist the chosen AR (a `users.stories_aspect TEXT`
  column or reuse a generic settings store). Card opens with the user's default.
- **Stream-copy optimization:** in the render pass, when `aspect == Original`
  **and** no other filter is required (no reframe), segment with `-c copy`
  (the segment muxer + `-c copy` cuts on existing keyframes; acceptable for
  "original" — document the keyframe-boundary caveat). Otherwise the normal
  re-encode pass.

### Phase B — Setup wizard

- **Entry:** `downloads:stories:{id}` (first time, no saved default) → wizard;
  config-card button `👁 Показать все AR` → `downloads:stories:wiz:{id}` → wizard.
- **Step 1 — contact sheet:** extract one frame at the clip midpoint
  (`ffmpeg -ss {mid} -i src -frames:v 1 frame.png`), then compose ONE image
  showing that frame center-cropped to each AR (9:16/1:1/4:5/16:9) side by side
  with labels. Compose via the `image` crate (already a dep in `dora`/core? — if
  not in dorabot, use one ffmpeg `hstack`/`tile` pass over per-AR crops of the
  single frame; still one cheap op, no video). Send as a photo with an inline
  keyboard: one button per AR → `downloads:stories:wsel:{id}:{arTok}`.
- **Step 2 — sample clip:** on `wsel` → render ONE short sample: 5 s from the
  midpoint, scaled to 540-wide-equivalent for the AR, `-preset veryfast -crf 28`,
  no audio → seconds. Send as video with buttons: `✅ Использовать`
  (`downloads:stories:wok:{id}:{arTok}`) / `↩ К сетке`
  (`downloads:stories:wiz:{id}`).
- **Step 3 — confirm:** `wok` → save AR as the user's default → open the normal
  Stories config card pre-set to that AR.
- **Bounding (anti-runaway):** Step 1 = 1 still-frame + 1 compose (no encode).
  Step 2 = exactly 1 short low-res clip per tap. Reuse `STORIES_FFMPEG_TIMEOUT`
  with a much shorter per-sample budget; the existing 20-min source cap still
  guards. No path generates N video encodes at once.

## Data flow

Card/wizard callbacks are stateless (settings encoded in callback data) — same
pattern as the existing card. The only new persistent state is the per-user
default AR. The wizard reuses the existing source-resolution helper
(`get_download_history_entry` + the Bot API/MTProto file fetch) — the source is
fetched once and reused for the still frame and the sample clip within a wizard
session (temp dir, dropped on completion).

## Optimization summary

1. Contact sheet = single still-frame op (no video encode) — cheapest preview.
2. Sample clip = 5 s · low-res · `veryfast`/`crf 28` · no audio → seconds.
3. `Original` + no reframe → `-c copy` segmenting (no re-encode) in the main render.
4. Hard bound: ≤1 image + ≤1 sample clip per wizard interaction → no runaway.

## Error handling

- Frame extraction / compose fails → fall back to text-only AR chooser (buttons
  without the preview image); log a warning, never panic.
- Sample clip fails → tell the user, keep them on the grid.
- Unknown/legacy callback tokens → default AR 9:16 (parse is tolerant).
- All ffmpeg calls bounded by timeout; temp dir always cleaned (RAII guard, as now).

## Testing

Pure unit tests (no ffmpeg/Telegram I/O):
- `AspectRatio::dims/token/from_token/label` round-trips for all 5.
- `StorySettings` token round-trip incl. AR; legacy tokens (no AR char) → 9:16.
- Filter builder: crop filter contains `crop={w}:{h}` and
  `force_original_aspect_ratio=increase` for each AR; `Original` → no crop/scale.
- Stream-copy decision: `Original`+no-reframe → `-c copy` args; others → re-encode.
- Wizard callback parsing: `wiz`/`wsel:{ar}`/`wok:{ar}` shapes.

ffmpeg behavior (crop/sample/contact-sheet) is smoke-tested on Railway before
commit per the CLAUDE.md ffmpeg rule (new filter strings).

## Versioning

MINOR feature → next beta after the current train.

## Phasing for the plan

1. **Phase A** — AspectRatio + generalized filter + card AR row + per-user
   default + Original stream-copy + tests. Shippable alone (AR works via card).
2. **Phase B** — wizard (contact sheet → sample → save default) + callbacks +
   i18n + tests, on top of A.
