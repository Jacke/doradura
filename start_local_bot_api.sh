#!/bin/bash

# Скрипт для запуска локального Telegram Bot API сервера
# Использование: ./start_local_bot_api.sh

set -e

# Цвета для вывода
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}🚀 Запуск локального Telegram Bot API сервера${NC}"

# Проверяем наличие Docker
if ! command -v docker &> /dev/null; then
    echo -e "${RED}❌ Docker не установлен!${NC}"
    echo "Установите Docker: https://docs.docker.com/get-docker/"
    exit 1
fi

# Проверяем наличие docker-compose
if ! command -v docker-compose &> /dev/null && ! docker compose version &> /dev/null; then
    echo -e "${RED}❌ docker-compose не установлен!${NC}"
    exit 1
fi

# Проверяем наличие .env файла с API_ID и API_HASH
if [ ! -f .env.bot-api ]; then
    echo -e "${YELLOW}⚠️  Файл .env.bot-api не найден${NC}"
    echo "Создаю шаблон .env.bot-api..."
    cat > .env.bot-api << EOF
# Telegram API credentials
# Получите их на https://my.telegram.org
API_ID=YOUR_API_ID_HERE
API_HASH=YOUR_API_HASH_HERE
EOF
    echo -e "${YELLOW}📝 Пожалуйста, заполните .env.bot-api своими данными:${NC}"
    echo "   1. Откройте https://my.telegram.org"
    echo "   2. Получите API_ID и API_HASH"
    echo "   3. Отредактируйте .env.bot-api"
    exit 1
fi

# Загружаем переменные окружения
source .env.bot-api

# Проверяем, что API_ID и API_HASH установлены
if [ "$API_ID" == "YOUR_API_ID_HERE" ] || [ -z "$API_ID" ]; then
    echo -e "${RED}❌ API_ID не установлен в .env.bot-api${NC}"
    exit 1
fi

if [ "$API_HASH" == "YOUR_API_HASH_HERE" ] || [ -z "$API_HASH" ]; then
    echo -e "${RED}❌ API_HASH не установлен в .env.bot-api${NC}"
    exit 1
fi

# Проверяем, не запущен ли уже контейнер
if docker ps | grep -q telegram-bot-api; then
    echo -e "${YELLOW}⚠️  Контейнер telegram-bot-api уже запущен${NC}"
    echo "Остановить и перезапустить? (y/n)"
    read -r answer
    if [ "$answer" == "y" ] || [ "$answer" == "Y" ]; then
        echo "Останавливаю существующий контейнер..."
        docker-compose -f docker-compose.bot-api.yml down
    else
        echo "Выход..."
        exit 0
    fi
fi

# Создаем директорию для данных
mkdir -p bot-api-data

# Запускаем сервер
echo -e "${GREEN}📦 Запускаю Docker контейнер...${NC}"
if docker compose version &> /dev/null; then
    docker compose -f docker-compose.bot-api.yml up -d
else
    docker-compose -f docker-compose.bot-api.yml up -d
fi

# Ждем запуска сервера
echo -e "${YELLOW}⏳ Ожидание запуска сервера (10 секунд)...${NC}"
sleep 10

# Проверяем статус
if docker ps | grep -q telegram-bot-api; then
    echo -e "${GREEN}✅ Сервер запущен!${NC}"
    echo ""
    echo "📋 Информация:"
    echo "   - URL: http://localhost:8081"
    echo "   - Логи: docker logs -f telegram-bot-api"
    echo "   - Остановка: docker-compose -f docker-compose.bot-api.yml down"
    echo ""
    echo "🔧 Настройте бота:"
    echo "   export BOT_API_URL=http://localhost:8081"
    echo "   или добавьте в .env:"
    echo "   BOT_API_URL=http://localhost:8081"
    echo ""
    
    # Проверяем доступность сервера
    echo -e "${YELLOW}🔍 Проверяю доступность сервера...${NC}"
    if curl -s http://localhost:8081/health > /dev/null 2>&1; then
        echo -e "${GREEN}✅ Сервер доступен!${NC}"
    else
        echo -e "${YELLOW}⚠️  Сервер запущен, но healthcheck не отвечает${NC}"
        echo "   Это нормально, если сервер еще загружается. Подождите немного."
    fi
else
    echo -e "${RED}❌ Не удалось запустить сервер${NC}"
    echo "Проверьте логи: docker logs telegram-bot-api"
    exit 1
fi

