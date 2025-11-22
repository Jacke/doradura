#!/bin/bash

# Скрипт для обновления yt-dlp до последней версии
# Использование: ./update_ytdlp.sh

set -e

echo "======================================"
echo "🔄 Обновление yt-dlp"
echo "======================================"
echo ""

# Проверка текущей версии
echo "Текущая версия:"
yt-dlp --version || echo "yt-dlp не установлен"
echo ""

# Попытка обновления через самого yt-dlp
echo "Попытка обновления через yt-dlp -U..."
if yt-dlp -U 2>&1 | tee /tmp/ytdlp_update.log; then
    echo ""
    echo "✅ Обновление через yt-dlp -U успешно!"
else
    echo ""
    echo "⚠️  Не удалось обновить через yt-dlp -U"
    echo ""
    echo "Попытка обновления через pip3..."
    
    # Попытка через pip3
    if pip3 install -U yt-dlp --break-system-packages 2>&1; then
        echo "✅ Обновление через pip3 успешно!"
    else
        echo ""
        echo "⚠️  Не удалось обновить через pip3"
        echo ""
        echo "Попытка через pip..."
        
        # Попытка через pip
        if pip install -U yt-dlp --break-system-packages 2>&1; then
            echo "✅ Обновление через pip успешно!"
        else
            echo ""
            echo "❌ Не удалось обновить yt-dlp"
            echo ""
            echo "Ручная установка:"
            echo "  macOS: brew upgrade yt-dlp"
            echo "  Linux: sudo curl -L https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp -o /usr/local/bin/yt-dlp"
            echo "         sudo chmod a+rx /usr/local/bin/yt-dlp"
            exit 1
        fi
    fi
fi

echo ""
echo "======================================"
echo "Новая версия:"
yt-dlp --version
echo "======================================"
echo ""

# Тестирование с cookies
echo "🧪 Тестирование с YouTube..."
if [ -f "youtube_cookies.txt" ]; then
    if yt-dlp --cookies youtube_cookies.txt --extractor-args "youtube:player_client=android,web" --print "%(title)s" "https://www.youtube.com/watch?v=dQw4w9WgXcQ" &>/dev/null; then
        echo "✅ YouTube работает с новой версией!"
    else
        echo "⚠️  Проблемы с YouTube, но yt-dlp обновлен"
    fi
else
    echo "ℹ️  Файл youtube_cookies.txt не найден, пропуск теста"
fi

echo ""
echo "✅ Готово! Перезапусти бота для применения изменений."

