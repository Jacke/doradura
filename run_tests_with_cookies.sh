#!/bin/bash
# Скрипт для запуска тестов с cookies
# Usage: ./run_tests_with_cookies.sh

set -e

# Получаем абсолютный путь к директории проекта
PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COOKIES_FILE="${PROJECT_DIR}/youtube_cookies.txt"

# Цвета
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

echo -e "${GREEN}╔════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║         ЗАПУСК ТЕСТОВ С COOKIES                                ║${NC}"
echo -e "${GREEN}╚════════════════════════════════════════════════════════════════╝${NC}"
echo ""

# Проверяем наличие файла cookies
if [ ! -f "$COOKIES_FILE" ]; then
    echo -e "${RED}❌ ОШИБКА: Файл cookies не найден: ${COOKIES_FILE}${NC}"
    echo ""
    echo "Следуйте инструкции в QUICK_FIX.md для экспорта cookies"
    exit 1
fi

# Проверяем что файл не пустой
if [ ! -s "$COOKIES_FILE" ]; then
    echo -e "${RED}❌ ОШИБКА: Файл cookies пустой: ${COOKIES_FILE}${NC}"
    echo ""
    echo "Экспортируйте cookies заново (см. QUICK_FIX.md)"
    exit 1
fi

echo -e "${GREEN}✅ Найден файл cookies: ${COOKIES_FILE}${NC}"
echo -e "${GREEN}✅ Размер файла: $(du -h "$COOKIES_FILE" | cut -f1)${NC}"
echo ""

# Устанавливаем переменную окружения
export YTDL_COOKIES_FILE="$COOKIES_FILE"

echo -e "${YELLOW}▶ Установлена переменная: YTDL_COOKIES_FILE=${COOKIES_FILE}${NC}"
echo ""

# Запускаем тесты
echo -e "${YELLOW}▶ Запуск диагностики...${NC}"
echo ""

./test_ytdlp.sh diagnostics

echo ""
echo -e "${GREEN}╔════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║  Если видите '✅ Файл существует' - cookies настроены!         ║${NC}"
echo -e "${GREEN}║  Теперь можете запустить тест скачивания:                      ║${NC}"
echo -e "${GREEN}║  YTDL_COOKIES_FILE=${COOKIES_FILE} ./test_ytdlp.sh download     ║${NC}"
echo -e "${GREEN}╚════════════════════════════════════════════════════════════════╝${NC}"

