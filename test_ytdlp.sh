#!/bin/bash
# Скрипт для быстрого запуска тестов yt-dlp
# Usage: ./test_ytdlp.sh [test_name]

set -e

# Цвета для вывода
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}"
echo "╔════════════════════════════════════════════════════════════════╗"
echo "║             ТЕСТИРОВАНИЕ СИСТЕМЫ СКАЧИВАНИЯ yt-dlp             ║"
echo "╚════════════════════════════════════════════════════════════════╝"
echo -e "${NC}"

# Функция для запуска теста
run_test() {
    local test_name=$1
    local ignore_flag=$2
    
    echo -e "\n${YELLOW}▶ Запуск теста: ${test_name}${NC}\n"
    
    if [ "$ignore_flag" = "--ignored" ]; then
        cargo test --test ytdlp_integration_test "${test_name}" -- --nocapture --test-threads=1 --ignored
    else
        cargo test --test ytdlp_integration_test "${test_name}" -- --nocapture --test-threads=1
    fi
    
    if [ $? -eq 0 ]; then
        echo -e "\n${GREEN}✅ Тест ${test_name} успешно пройден${NC}"
    else
        echo -e "\n${RED}❌ Тест ${test_name} провален${NC}"
        exit 1
    fi
}

# Если передан аргумент - запускаем конкретный тест
if [ $# -eq 1 ]; then
    case $1 in
        "diagnostics"|"diag")
            run_test "test_full_diagnostics"
            ;;
        "install"|"installed")
            run_test "test_ytdlp_installed"
            ;;
        "version")
            run_test "test_ytdlp_version"
            ;;
        "cookies")
            run_test "test_cookies_configuration"
            ;;
        "metadata")
            run_test "test_ytdlp_get_metadata" "--ignored"
            ;;
        "download"|"audio")
            run_test "test_ytdlp_download_audio" "--ignored"
            ;;
        "invalid")
            run_test "test_ytdlp_invalid_url" "--ignored"
            ;;
        "quality"|"qualities")
            run_test "test_ytdlp_different_qualities" "--ignored"
            ;;
        "all-basic")
            echo -e "${BLUE}Запуск всех базовых тестов (без скачивания)${NC}"
            run_test "test_ytdlp_installed"
            run_test "test_ytdlp_version"
            run_test "test_cookies_configuration"
            run_test "test_full_diagnostics"
            ;;
        "all-download")
            echo -e "${BLUE}Запуск всех тестов со скачиванием (требует интернет)${NC}"
            run_test "test_ytdlp_get_metadata" "--ignored"
            run_test "test_ytdlp_download_audio" "--ignored"
            run_test "test_ytdlp_invalid_url" "--ignored"
            run_test "test_ytdlp_different_qualities" "--ignored"
            ;;
        "all")
            echo -e "${BLUE}Запуск ВСЕХ тестов${NC}"
            run_test "test_ytdlp_installed"
            run_test "test_ytdlp_version"
            run_test "test_cookies_configuration"
            run_test "test_full_diagnostics"
            run_test "test_ytdlp_get_metadata" "--ignored"
            run_test "test_ytdlp_download_audio" "--ignored"
            run_test "test_ytdlp_invalid_url" "--ignored"
            run_test "test_ytdlp_different_qualities" "--ignored"
            ;;
        "help"|"-h"|"--help")
            echo -e "${GREEN}Использование:${NC}"
            echo "  ./test_ytdlp.sh [test_name]"
            echo ""
            echo -e "${GREEN}Доступные тесты:${NC}"
            echo "  diagnostics, diag     - Полная диагностика системы (рекомендуется запустить первым)"
            echo "  install, installed    - Проверка установки yt-dlp и ffmpeg"
            echo "  version              - Проверка версии yt-dlp"
            echo "  cookies              - Проверка конфигурации cookies"
            echo "  metadata             - Получение метаданных видео (требует интернет)"
            echo "  download, audio      - Тест скачивания аудио (требует интернет)"
            echo "  invalid              - Тест обработки невалидного URL (требует интернет)"
            echo "  quality, qualities   - Тест разных качеств скачивания (требует интернет)"
            echo ""
            echo -e "${GREEN}Групповые тесты:${NC}"
            echo "  all-basic            - Все базовые тесты (без скачивания)"
            echo "  all-download         - Все тесты со скачиванием"
            echo "  all                  - ВСЕ тесты"
            echo ""
            echo -e "${GREEN}Примеры:${NC}"
            echo "  ./test_ytdlp.sh diagnostics    # Быстрая проверка системы"
            echo "  ./test_ytdlp.sh download        # Полный тест скачивания"
            echo "  ./test_ytdlp.sh all-basic       # Все тесты без интернета"
            echo ""
            echo -e "${YELLOW}💡 Совет: Запустите сначала 'diagnostics' чтобы проверить готовность системы${NC}"
            exit 0
            ;;
        *)
            echo -e "${RED}❌ Неизвестный тест: $1${NC}"
            echo "Запустите './test_ytdlp.sh help' для списка доступных тестов"
            exit 1
            ;;
    esac
else
    # Если аргументов нет - запускаем быструю диагностику
    echo -e "${YELLOW}💡 Не указан тест - запускаем полную диагностику${NC}"
    echo -e "${YELLOW}   Для списка доступных тестов: ./test_ytdlp.sh help${NC}\n"
    run_test "test_full_diagnostics"
fi

echo -e "\n${BLUE}"
echo "╔════════════════════════════════════════════════════════════════╗"
echo "║                  ТЕСТИРОВАНИЕ ЗАВЕРШЕНО                        ║"
echo "╚════════════════════════════════════════════════════════════════╝"
echo -e "${NC}"

