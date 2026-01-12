#!/bin/bash
# Script to update yt-dlp to the latest version
# Usage: ./update_ytdlp.sh

set -e

echo "🔄 Updating yt-dlp"

echo "Current version:"
yt-dlp --version || echo "yt-dlp not installed"

echo "Trying yt-dlp -U..."
if yt-dlp -U; then
    echo "✅ Updated via yt-dlp -U successfully!"
else
    echo "⚠️  Failed to update via yt-dlp -U"
    echo "Trying pip3..."
    if pip3 install -U yt-dlp; then
        echo "✅ Updated via pip3 successfully!"
    else
        echo "⚠️  Failed via pip3"
        echo "Trying pip..."
        if pip install -U yt-dlp; then
            echo "✅ Updated via pip successfully!"
        else
            echo "❌ Failed to update yt-dlp"
            echo "Manual install: https://github.com/yt-dlp/yt-dlp#installation"
        fi
    fi
fi

echo "New version:"
yt-dlp --version || true

echo "🧪 Testing YouTube..."
if [ -f youtube_cookies.txt ]; then
    if yt-dlp --cookies youtube_cookies.txt --print "%(title)s" "https://www.youtube.com/watch?v=dQw4w9WgXcQ" &>/dev/null; then
        echo "✅ YouTube works with the new version!"
    else
        echo "⚠️  Issues with YouTube, but yt-dlp updated"
    fi
else
    echo "ℹ️  youtube_cookies.txt not found, skipping test"
fi

echo "✅ Done! Restart the bot to apply changes."
