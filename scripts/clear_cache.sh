#!/bin/bash
# Script to clear metadata cache
# Use if videos are downloading with wrong titles

echo "🧹 Clearing metadata cache..."

# Check if bot is running
if pgrep -f doradura >/dev/null; then
    echo "⚠️  Bot is running!"
    echo "For a full cache clear, stop the bot first."
    read -p "Stop the bot? (y/N): " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        echo "Stopping bot..."
        pkill -f doradura
        echo "✅ Bot stopped"
    else
        echo "⚠️  Cache might not fully clear while bot runs"
    fi
fi

echo "Cache is stored in application memory."
echo "For a full clear, simply restart the bot:"
echo "  1. Stop the bot (Ctrl+C or pkill -f doradura)"
echo "  2. Start the bot again"
echo "After restart, all videos will get fresh metadata! ✨"
