#!/bin/bash

# Скрипт для запуска бота с cookies
# Использование: ./run_with_cookies.sh

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COOKIES_FILE="${SCRIPT_DIR}/youtube_cookies.txt"

echo "======================================"
echo "🚀 Запуск бота с YouTube cookies"
echo "======================================"
echo ""

# Проверка 1: Файл cookies существует
if [ ! -f "$COOKIES_FILE" ]; then
    echo "❌ Файл cookies не найден: $COOKIES_FILE"
    echo ""
    echo "Создай файл с cookies. См. YOUTUBE_COOKIES.md"
    exit 1
fi

echo "✅ Файл cookies найден: $COOKIES_FILE"
echo ""

# Проверка 2: Права доступа
PERMS=$(stat -f "%OLp" "$COOKIES_FILE" 2>/dev/null || stat -c "%a" "$COOKIES_FILE" 2>/dev/null)
if [ "$PERMS" != "600" ]; then
    echo "⚠️  Права доступа: $PERMS (рекомендуется: 600)"
    echo "   Установка безопасных прав..."
    chmod 600 "$COOKIES_FILE"
    echo "   ✅ Права установлены: 600"
fi
echo ""

# Проверка 3: Тестирование cookies
echo "🔍 Тестирование cookies с yt-dlp..."
if yt-dlp --cookies "$COOKIES_FILE" --print "%(title)s" "https://www.youtube.com/watch?v=dQw4w9WgXcQ" &>/dev/null; then
    echo "✅ Cookies работают!"
else
    echo "⚠️  Не удалось проверить cookies с yt-dlp"
    echo "   Бот будет запущен, но могут быть проблемы с YouTube"
fi
echo ""

# Установка переменной окружения
export YTDL_COOKIES_FILE="$COOKIES_FILE"

echo "======================================"
echo "Запуск бота..."
echo "======================================"
echo ""
echo "Переменные окружения:"
echo "  YTDL_COOKIES_FILE=$YTDL_COOKIES_FILE"
echo ""

# Запуск бота
cd "$SCRIPT_DIR"
cargo run --release

