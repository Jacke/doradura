#!/bin/bash

# Тестовый скрипт для проверки вывода прогресса yt-dlp
# Проверяет, какой формат вывода использует yt-dlp с текущими настройками

echo "Testing yt-dlp progress output format..."
echo "=========================================="

TEST_URL="https://www.youtube.com/watch?v=dQw4w9WgXcQ"

# Проверяем с --newline и android client
echo ""
echo "Test 1: With --newline and android client (current settings)"
echo "--------------------------------------------------------------"
yt-dlp \
    --newline \
    --extract-audio \
    --audio-format mp3 \
    --audio-quality 0 \
    --no-playlist \
    --extractor-args "youtube:player_client=android" \
    --no-check-certificate \
    -o "/tmp/test_download.mp3" \
    "$TEST_URL" \
    2>&1 | head -50

echo ""
echo "=========================================="
echo "Done! Check if you see lines like:"
echo "[download]  45.2% of 10.00MiB at 500.00KiB/s ETA 00:10"

