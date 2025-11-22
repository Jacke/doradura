#!/bin/bash

# Скрипт для проверки настройки YouTube cookies для yt-dlp
# Использование: ./check_cookies.sh [browser]
# Пример: ./check_cookies.sh firefox

set -e

BROWSER=${1:-${YTDL_COOKIES_BROWSER:-chrome}}
TEST_URL="https://www.youtube.com/watch?v=dQw4w9WgXcQ"

echo "======================================"
echo "Проверка YouTube cookies для yt-dlp"
echo "======================================"
echo ""

# Проверка 1: yt-dlp установлен
echo "✓ Проверка 1: yt-dlp установлен"
if ! command -v yt-dlp &> /dev/null; then
    echo "  ❌ yt-dlp не найден. Установи: pip3 install yt-dlp"
    exit 1
fi
echo "  ✅ yt-dlp найден: $(which yt-dlp)"
echo ""

# Проверка 2: Python зависимости
echo "✓ Проверка 2: Python зависимости"
if python3 -c "import keyring" 2>/dev/null; then
    echo "  ✅ keyring установлен"
else
    echo "  ⚠️  keyring не установлен (может не работать с Chrome/Chromium)"
    echo "     Установи: pip3 install keyring"
fi

if python3 -c "import Cryptodome" 2>/dev/null || python3 -c "import Crypto" 2>/dev/null; then
    echo "  ✅ pycryptodomex установлен"
else
    echo "  ⚠️  pycryptodomex не установлен (может не работать с Chrome/Chromium)"
    echo "     Установи: pip3 install pycryptodomex"
fi
echo ""

# Проверка 3: Браузер
echo "✓ Проверка 3: Тестирование с браузером '${BROWSER}'"
echo "  Попытка получить название видео с YouTube..."
echo ""

if yt-dlp --cookies-from-browser "${BROWSER}" --print "%(title)s" "${TEST_URL}" 2>/dev/null; then
    echo ""
    echo "  ✅✅✅ Отлично! Cookies работают с браузером '${BROWSER}'!"
    echo ""
    echo "Бот готов к работе с YouTube. Просто запусти:"
    echo "  export YTDL_COOKIES_BROWSER=${BROWSER}"
    echo "  cargo run --release"
else
    echo ""
    echo "  ❌ Не удалось получить cookies из '${BROWSER}'"
    echo ""
    echo "Возможные решения:"
    echo ""
    echo "1. Попробуй Firefox (работает лучше всего):"
    echo "   ./check_cookies.sh firefox"
    echo ""
    echo "2. Убедись, что браузер установлен и ты заходил на YouTube"
    echo ""
    echo "3. Попробуй другие браузеры:"
    echo "   - chrome"
    echo "   - firefox"
    echo "   - safari (только macOS)"
    echo "   - brave"
    echo ""
    echo "4. Экспортируй cookies вручную (см. YOUTUBE_COOKIES.md)"
    echo ""
    exit 1
fi

echo "======================================"
echo "Все проверки пройдены! 🎉"
echo "======================================"

