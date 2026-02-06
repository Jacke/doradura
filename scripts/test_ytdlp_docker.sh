#!/bin/sh
# Test yt-dlp with Railway-equivalent params inside Docker
# This verifies: deno, curl_cffi (--impersonate), android+web_music clients
set -e

echo "=== yt-dlp Docker Test ==="
echo ""

# Check tools
echo "1. Checking tools..."
echo "   yt-dlp: $(yt-dlp --version)"
echo "   deno: $(deno --version | head -1)"
echo "   curl_cffi: $(python3 -c 'import curl_cffi; print(curl_cffi.__version__)' 2>/dev/null || echo 'NOT INSTALLED')"
echo ""

# List available impersonate targets
echo "2. Available impersonate targets:"
yt-dlp --list-impersonate-targets 2>/dev/null | head -5
echo "   ..."
echo ""

# Test metadata extraction (no download)
TEST_URL="https://www.youtube.com/watch?v=jNQXAC9IVRw"
echo "3. Testing metadata extraction..."
TITLE=$(yt-dlp \
    --print "%(title)s|||%(uploader)s" \
    --no-playlist \
    --skip-download \
    --extractor-args "youtube:player_client=android,web_music;formats=missing_pot" \
    --js-runtimes deno \
    --impersonate Chrome-131:Android-14 \
    --no-check-certificate \
    "$TEST_URL" 2>/dev/null)

if [ -z "$TITLE" ]; then
    echo "   FAIL: Empty metadata"
    exit 1
fi
echo "   OK: $TITLE"
echo ""

# Test actual download (worst quality for speed)
echo "4. Testing video download (worst quality)..."
OUTPUT="/tmp/test_video.mp4"
yt-dlp \
    -f "worst[ext=mp4]/worst" \
    -o "$OUTPUT" \
    --no-playlist \
    --extractor-args "youtube:player_client=android,web_music;formats=missing_pot" \
    --js-runtimes deno \
    --impersonate Chrome-131:Android-14 \
    --sleep-requests 2 \
    --sleep-interval 3 \
    --max-sleep-interval 10 \
    --limit-rate 5M \
    --retries 15 \
    --fragment-retries 10 \
    --no-check-certificate \
    "$TEST_URL" 2>&1

if [ ! -f "$OUTPUT" ]; then
    echo "   FAIL: File not created"
    exit 1
fi

SIZE=$(stat -c%s "$OUTPUT" 2>/dev/null || stat -f%z "$OUTPUT" 2>/dev/null)
echo ""
echo "   OK: Downloaded $SIZE bytes"

# Cleanup
rm -f "$OUTPUT"

echo ""
echo "=== ALL TESTS PASSED ==="
