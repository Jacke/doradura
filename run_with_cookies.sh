#!/bin/bash

# Script to run the bot with YouTube cookies
# Usage: ./run_with_cookies.sh

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COOKIES_FILE="${SCRIPT_DIR}/youtube_cookies.txt"

echo "======================================"
echo "🚀 Starting bot with YouTube cookies"
echo "======================================"
echo ""

# Check 1: cookies file exists
if [ ! -f "$COOKIES_FILE" ]; then
    echo "❌ Cookies file not found: $COOKIES_FILE"
    echo ""
    echo "Create a cookies file. See YOUTUBE_COOKIES.md"
    exit 1
fi

echo "✅ Cookies file found: $COOKIES_FILE"
echo ""

# Check 2: permissions
PERMS=$(stat -f "%OLp" "$COOKIES_FILE" 2>/dev/null || stat -c "%a" "$COOKIES_FILE" 2>/dev/null)
if [ "$PERMS" != "600" ]; then
    echo "⚠️  Permissions: $PERMS (recommended: 600)"
    echo "   Setting secure permissions..."
    chmod 600 "$COOKIES_FILE"
    echo "   ✅ Permissions set to 600"
fi
echo ""

# Check 3: test cookies
echo "🔍 Testing cookies with yt-dlp..."
if yt-dlp --cookies "$COOKIES_FILE" --print "%(title)s" "https://www.youtube.com/watch?v=dQw4w9WgXcQ" &>/dev/null; then
    echo "✅ Cookies are valid!"
else
    echo "⚠️  Could not validate cookies with yt-dlp"
    echo "   Bot will start, but YouTube may fail"
fi
echo ""

# Export env var
export YTDL_COOKIES_FILE="$COOKIES_FILE"

echo "======================================"
echo "Starting bot..."
echo "======================================"
echo ""
echo "Environment variables:"
echo "  YTDL_COOKIES_FILE=$YTDL_COOKIES_FILE"
echo ""

# Run bot
cd "$SCRIPT_DIR"
cargo run --release
