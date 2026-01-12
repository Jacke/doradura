#!/bin/bash

# Script to verify YouTube cookies for yt-dlp
# Usage: ./check_cookies.sh [browser]
# Example: ./check_cookies.sh firefox

BROWSER=${1:-chrome}

echo "Checking YouTube cookies for yt-dlp"

# Check 1: yt-dlp installed
if ! command -v yt-dlp &>/dev/null; then
    echo "  ❌ yt-dlp not found. Install: pip3 install yt-dlp"
    exit 1
fi
echo "  ✅ yt-dlp found: $(which yt-dlp)"

# Check 2: Python deps
if python3 -c "import keyring" &>/dev/null; then
    echo "  ✅ keyring installed"
else
    echo "  ⚠️  keyring not installed (may be needed for Chrome/Chromium)"
    echo "     Install: pip3 install keyring"
fi

if python3 -c "import Cryptodome" &>/dev/null; then
    echo "  ✅ pycryptodomex installed"
else
    echo "  ⚠️  pycryptodomex not installed (may be needed for Chrome/Chromium)"
    echo "     Install: pip3 install pycryptodomex"
fi

# Check 3: Browser
echo "✓ Check 3: Testing with browser '${BROWSER}'"
echo "  Trying to read a YouTube title..."
if yt-dlp --cookies-from-browser "$BROWSER" --print "%(title)s" "https://www.youtube.com/watch?v=dQw4w9WgXcQ" &>/dev/null; then
    echo "  ✅✅✅ Great! Cookies work with browser '${BROWSER}'!"
    echo "Bot is ready for YouTube. Run:"
    echo "  export YTDL_COOKIES_BROWSER=${BROWSER}"
    echo "  cargo run --release"
    echo ""
    echo "If using fish shell:"
    echo "  set -x YTDL_COOKIES_BROWSER ${BROWSER}"
else
    echo "  ❌ Could not get cookies from '${BROWSER}'"
    echo "Possible fixes:"
    echo "1. Try Firefox (most reliable):"
    echo "   ./check_cookies.sh firefox"
    echo "2. Ensure the browser is installed and logged into YouTube"
    echo "3. Try other browsers:"
    echo "   ./check_cookies.sh chrome"
    echo "   ./check_cookies.sh brave"
    echo "   ./check_cookies.sh edge"
    echo "   - safari (macOS only)"
    echo "4. Export cookies manually (see YOUTUBE_COOKIES.md)"
    exit 1
fi

echo "All checks passed! 🎉"
