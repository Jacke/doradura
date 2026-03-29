#!/bin/sh
# Test yt-dlp download args on Railway container before deploying.
#
# Usage (from local machine):
#   railway ssh --service doradura -- sh /app/scripts/test-ytdlp-args.sh
#
# Or copy+paste into railway ssh session.
#
# Tests BOTH mp3 and mp4 with experimental args (matching our Rust code).
# Exit code 0 = all tests passed, non-zero = something broke.

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

TEST_URL="https://youtu.be/jNQXAC9IVRw"  # "Me at the zoo" - short, always available
COOKIES="/data/youtube_cookies.txt"
OUTDIR="/tmp/ytdlp-test-$$"
PASS=0
FAIL=0

mkdir -p "$OUTDIR"
cleanup() { rm -rf "$OUTDIR"; }
trap cleanup EXIT

ok()   { PASS=$((PASS+1)); printf "${GREEN}PASS${NC}: %s\n" "$1"; }
fail() { FAIL=$((FAIL+1)); printf "${RED}FAIL${NC}: %s\n" "$1"; }
info() { printf "${YELLOW}TEST${NC}: %s\n" "$1"; }

# ─── Common args (mirrors build_common_args in ytdlp.rs) ───

# Experimental mode (no rate limit, 10MB chunks, no --concurrent-fragments here)
COMMON_EXP="--newline --force-overwrites --no-playlist --age-limit 99 \
  --fragment-retries 10 --socket-timeout 30 \
  --http-chunk-size 10485760 --retries 15 \
  --retry-sleep http:exp=1:30 --retry-sleep fragment:exp=1:30 \
  --throttled-rate 100K"

# Conservative mode (rate limit + sleep, 2MB chunks)
COMMON_SAFE="--newline --force-overwrites --no-playlist --age-limit 99 \
  --concurrent-fragments 1 --fragment-retries 10 --socket-timeout 30 \
  --http-chunk-size 2097152 \
  --sleep-requests 2 --sleep-interval 3 --max-sleep-interval 10 \
  --limit-rate 5M \
  --retry-sleep http:exp=1:30 --retry-sleep fragment:exp=1:30 --retries 15"

# YouTube auth args (mirrors add_cookies_args_with_proxy)
YT_AUTH="--extractor-args youtubepot-bgutilhttp:base_url=http://127.0.0.1:4416 \
  --cookies $COOKIES \
  --extractor-args youtube:player_client=default"

RUNTIME="--js-runtimes deno --no-check-certificate"

echo "============================================"
echo "  yt-dlp args smoke test"
echo "  URL: $TEST_URL"
echo "  yt-dlp: $(yt-dlp --version)"
echo "============================================"
echo ""

# ─── TEST 1: MP3 experimental (the one that broke) ───
info "MP3 + experimental + -N 4"
OUT1="$OUTDIR/test1.mp3"
if yt-dlp -o "$OUT1" $COMMON_EXP \
  --extract-audio --audio-format mp3 --audio-quality 0 \
  --add-metadata --embed-thumbnail \
  $YT_AUTH $RUNTIME \
  -N 4 \
  --postprocessor-args "ffmpeg:-acodec libmp3lame -b:a 320k" \
  "$TEST_URL" 2>&1 | tail -3; then
  [ -f "$OUT1" ] && [ "$(stat -f%z "$OUT1" 2>/dev/null || stat -c%s "$OUT1")" -gt 10000 ] \
    && ok "MP3 experimental downloaded ($(du -h "$OUT1" | cut -f1))" \
    || fail "MP3 experimental: file missing or too small"
else
  fail "MP3 experimental: yt-dlp exited with error"
fi
echo ""

# ─── TEST 2: MP3 safe mode (no -N) ───
info "MP3 + safe mode (no -N)"
OUT2="$OUTDIR/test2.mp3"
if yt-dlp -o "$OUT2" $COMMON_SAFE \
  --extract-audio --audio-format mp3 --audio-quality 0 \
  --add-metadata --embed-thumbnail \
  $YT_AUTH $RUNTIME \
  --postprocessor-args "ffmpeg:-acodec libmp3lame -b:a 320k" \
  "$TEST_URL" 2>&1 | tail -3; then
  [ -f "$OUT2" ] && [ "$(stat -f%z "$OUT2" 2>/dev/null || stat -c%s "$OUT2")" -gt 10000 ] \
    && ok "MP3 safe downloaded ($(du -h "$OUT2" | cut -f1))" \
    || fail "MP3 safe: file missing or too small"
else
  fail "MP3 safe: yt-dlp exited with error"
fi
echo ""

# ─── TEST 3: MP4 experimental ───
info "MP4 + experimental + -N 4"
OUT3="$OUTDIR/test3.mp4"
if yt-dlp -o "$OUT3" $COMMON_EXP \
  --format "bv*[height<=480][vcodec^=avc1]+ba[acodec^=mp4a]/best[ext=mp4]/best" \
  --merge-output-format mp4 \
  --postprocessor-args "Merger:-movflags +faststart" \
  $YT_AUTH $RUNTIME \
  -N 4 \
  "$TEST_URL" 2>&1 | tail -3; then
  [ -f "$OUT3" ] && [ "$(stat -f%z "$OUT3" 2>/dev/null || stat -c%s "$OUT3")" -gt 10000 ] \
    && ok "MP4 experimental downloaded ($(du -h "$OUT3" | cut -f1))" \
    || fail "MP4 experimental: file missing or too small"
else
  fail "MP4 experimental: yt-dlp exited with error"
fi
echo ""

# ─── TEST 4: MP4 safe mode ───
info "MP4 + safe mode (no -N)"
OUT4="$OUTDIR/test4.mp4"
if yt-dlp -o "$OUT4" $COMMON_SAFE \
  --format "bv*[height<=480][vcodec^=avc1]+ba[acodec^=mp4a]/best[ext=mp4]/best" \
  --merge-output-format mp4 \
  --postprocessor-args "Merger:-movflags +faststart" \
  $YT_AUTH $RUNTIME \
  "$TEST_URL" 2>&1 | tail -3; then
  [ -f "$OUT4" ] && [ "$(stat -f%z "$OUT4" 2>/dev/null || stat -c%s "$OUT4")" -gt 10000 ] \
    && ok "MP4 safe downloaded ($(du -h "$OUT4" | cut -f1))" \
    || fail "MP4 safe: file missing or too small"
else
  fail "MP4 safe: yt-dlp exited with error"
fi
echo ""

# ─── TEST 5: MP3 without cookies (non-YouTube path) ───
info "MP3 + no cookies (non-YouTube simulation)"
OUT5="$OUTDIR/test5.mp3"
if yt-dlp -o "$OUT5" $COMMON_EXP \
  --extract-audio --audio-format mp3 --audio-quality 0 \
  --add-metadata \
  --extractor-args "youtube:player_client=default;formats=missing_pot" \
  $RUNTIME \
  -N 4 \
  --postprocessor-args "ffmpeg:-acodec libmp3lame -b:a 320k" \
  "$TEST_URL" 2>&1 | tail -3; then
  [ -f "$OUT5" ] && [ "$(stat -f%z "$OUT5" 2>/dev/null || stat -c%s "$OUT5")" -gt 10000 ] \
    && ok "MP3 no-cookies downloaded ($(du -h "$OUT5" | cut -f1))" \
    || fail "MP3 no-cookies: file missing or too small"
else
  # Expected to fail on Railway (datacenter IP blocked), that's OK
  ok "MP3 no-cookies: failed as expected (datacenter IP)"
fi
echo ""

# ─── Summary ───
echo "============================================"
printf "  Results: ${GREEN}%d passed${NC}, ${RED}%d failed${NC}\n" "$PASS" "$FAIL"
echo "============================================"

[ "$FAIL" -eq 0 ] && exit 0 || exit 1
