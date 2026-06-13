# Instagram Stories — Enhancements Design

Date: 2026-06-13
Module: `crates/dorabot/src/telegram/downloads/stories.rs`

## Context

Instagram Stories already exists (v0.51.0-alpha.31): button on any downloaded MP4 →
reframe to 9:16 1080×1920 with a blurred fill background → split into 60 s segments →
send each as a portrait video. Verified working (6 unit tests pass; ffmpeg filter graph
produces correct 1080×1920 segments on the Railway prod box).

This design adds user-facing **options** while keeping the one-tap path fast.

## Goals (the 4 gaps)

1. **Reframe mode choice**: keep current blurred-fill (`blur`) AND add `crop` (zoom to
   fill 9:16, cropping the edges — the "native" Stories look).
2. **Raise the duration cap**: 10 min → 20 min (`MAX_TOTAL_SECS` 600 → 1200), still
   trim-from-start with a warning. Not removed outright: an unbounded encode on a shared
   box risks a runaway/outage (per CLAUDE.md caution). 20 min stays within the 30 min
   ffmpeg timeout even at the slow preset.
3. **Quality choice**: `std` (CRF 20, preset medium, AAC 192k — today's behaviour) and
   `max` (CRF 18, preset slow, AAC 256k).
4. **Segment length choice**: 15 / 30 / 60 s.

## UX — config card (stateless, no DB)

Clicking **📱 Instagram Stories** (`downloads:stories:{id}`) now opens a small config
card instead of running immediately. The card edits itself in place as options toggle,
so it never spams the chat. One tap on **▶️ Создать** runs with the shown settings; the
defaults match today's behaviour (blur / 60 s / std), so it stays effectively one extra tap.

### Callback scheme (parsed via existing `splitn(6, ':')`)

- `downloads:stories:{id}` — entry → render card with default flags `b60s`.
- `downloads:stories:cfg:{id}:{flags}` — toggle → `edit_md_kb` re-render.
- `downloads:stories:go:{id}:{flags}` — run render + send.

`flags` is a compact token: `<mode char><seg digits><quality char>`, e.g. `b60s`, `c30m`.
- mode: `b` = blur, `c` = crop
- seg: `15` | `30` | `60`
- quality: `s` = std, `m` = max

Parse is tolerant (unknown → defaults). Encoding keeps callback_data well under 64 bytes.

`stories::handle` branches on `parts[2]`: `"cfg"` / `"go"` → new flow; otherwise numeric
→ render card (backward compatible).

### Card keyboard

```
[ ● 🌫 Размытый фон ] [ ○ 🔍 Обрезать ]
[ ○ 15с ] [ ○ 30с ] [ ● 60с ]
[ ● Стандарт ] [ ○ Максимум ]
[ ▶️ Создать стори ]
[ ❌ Отмена ]
```
Active option marked with `●`, inactive `○`. Each button's callback carries the resulting
flags. Cancel → `downloads:cancel`.

## ffmpeg

`build_stories_cmd(input, output_pattern, capped, mode, seg_secs, quality)`:

- **blur** (unchanged): `split=2[bg][fg]` → bg cover+crop+`boxblur=28:2`+`eq=brightness=-0.07`,
  fg fit, `overlay` centre.
- **crop**: `[0:v]scale=W:H:force_original_aspect_ratio=increase,crop=W:H,setsar=1[v]`.
- **std**: `-crf 20 -preset medium -b:a 192k`.
- **max**: `-crf 18 -preset slow -b:a 256k`.
- seg: `-force_key_frames expr:gte(t,n_forced*{seg})`, `-segment_time {seg}`.

## i18n (en/ru/fr/de)

New keys: `stories-config-title` (`$title`), `stories-mode-blur`, `stories-mode-crop`,
`stories-quality-std`, `stories-quality-max`, `stories-create`, `stories-cancel`.
Seg labels (`15с/30с/60с`) are literal/universal.

## Tests

Unit tests asserting command construction: crop filter (single chain, has `crop`, no
`boxblur`); blur filter unchanged; max → crf 18 / preset slow / 256k; std → crf 20 /
medium / 192k; seg 15/30 propagate to both `force_key_frames` and `-segment_time`;
`parse_flags`/`encode_flags` round-trip and tolerate junk.

## Verification

`cargo check` + `cargo test -p doradura --lib stories::` + Railway ffmpeg smoke test of
BOTH new filters (crop, and max quality) before any commit (CLAUDE.md hard rule).

## Out of scope

Per-user persisted preferences (DB), arbitrary segment lengths, region selection
(trim-from-start kept). Can follow later.
