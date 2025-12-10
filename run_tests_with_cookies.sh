#!/bin/bash

# Script to run tests with cookies
# Usage: ./run_tests_with_cookies.sh

set -e

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COOKIES_FILE="${PROJECT_DIR}/youtube_cookies.txt"

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

clear
printf "${GREEN}║         RUNNING TESTS WITH COOKIES                               ║${NC}\n"

# Check cookies file
if [ ! -f "$COOKIES_FILE" ]; then
    printf "${RED}❌ ERROR: Cookies file not found: ${COOKIES_FILE}${NC}\n"
    echo "See QUICK_FIX.md for export instructions"
    exit 1
fi

# Ensure file is not empty
if [ ! -s "$COOKIES_FILE" ]; then
    printf "${RED}❌ ERROR: Cookies file is empty: ${COOKIES_FILE}${NC}\n"
    echo "Re-export cookies (see QUICK_FIX.md)"
    exit 1
fi

printf "${GREEN}✅ Found cookies file: ${COOKIES_FILE}${NC}\n"
printf "${GREEN}✅ File size: $(du -h "$COOKIES_FILE" | cut -f1)${NC}\n"

# Set env var
export YTDL_COOKIES_FILE="$COOKIES_FILE"
printf "${YELLOW}▶ Set env: YTDL_COOKIES_FILE=${COOKIES_FILE}${NC}\n"

# Run tests
printf "${YELLOW}▶ Running diagnostics...${NC}\n"
./test_ytdlp.sh diagnostics

printf "${GREEN}║  If you see '✅ File exists' - cookies are configured!          ║${NC}\n"
printf "${GREEN}║  You can now run the download test:                             ║${NC}\n"
printf "${GREEN}║     ./test_ytdlp.sh download                                    ║${NC}\n"
