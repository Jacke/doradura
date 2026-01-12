#!/bin/bash
# Quick runner for yt-dlp tests

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

print_banner() {
    echo -e "${GREEN}║             YT-DLP DOWNLOAD SYSTEM TESTING                     ║${NC}"
}

run_test() {
    local test_name="$1"
    echo -e "\n${YELLOW}▶ Running test: ${test_name}${NC}\n"
    cargo test --test ytdlp_integration_test "$test_name" -- --nocapture --test-threads=1
    if [ $? -eq 0 ]; then
        echo -e "\n${GREEN}✅ Test ${test_name} passed${NC}"
    else
        echo -e "\n${RED}❌ Test ${test_name} failed${NC}"
        exit 1
    fi
}

if [ $# -gt 0 ]; then
    case "$1" in
        diagnostics|diag)
            run_test test_full_diagnostics ;;
        install|installed)
            run_test test_ytdlp_installed ;;
        version)
            run_test test_ytdlp_version ;;
        cookies)
            run_test test_cookies_configuration ;;
        metadata)
            run_test test_ytdlp_get_metadata ;;
        download|audio)
            run_test test_ytdlp_download_audio ;;
        invalid)
            run_test test_ytdlp_invalid_url ;;
        quality|qualities)
            run_test test_ytdlp_different_qualities ;;
        all-basic)
            echo -e "${BLUE}Running all basic tests (offline)${NC}"
            run_test test_full_diagnostics
            run_test test_ytdlp_installed
            run_test test_ytdlp_version
            run_test test_cookies_configuration
            ;;
        all-download)
            echo -e "${BLUE}Running all download tests${NC}"
            run_test test_ytdlp_get_metadata
            run_test test_ytdlp_download_audio
            run_test test_ytdlp_invalid_url
            run_test test_ytdlp_different_qualities
            ;;
        all)
            echo -e "${BLUE}Running ALL tests${NC}"
            run_test test_full_diagnostics
            run_test test_ytdlp_installed
            run_test test_ytdlp_version
            run_test test_cookies_configuration
            run_test test_ytdlp_get_metadata
            run_test test_ytdlp_download_audio
            run_test test_ytdlp_invalid_url
            run_test test_ytdlp_different_qualities
            ;;
        help|-h|--help)
            echo -e "${GREEN}Usage:${NC}"
            echo "  ./test_ytdlp.sh <test>"
            echo -e "${GREEN}Available tests:${NC}"
            echo "  diagnostics, diag   - Full system diagnostics"
            echo "  install, installed  - Check yt-dlp/ffmpeg installed"
            echo "  version             - Check yt-dlp version"
            echo "  cookies             - Check cookies configuration"
            echo "  metadata            - Fetch metadata (needs internet)"
            echo "  download, audio     - Audio download (needs internet)"
            echo "  invalid             - Invalid URL handling (needs internet)"
            echo "  quality, qualities  - Quality tests (needs internet)"
            echo -e "${GREEN}Groups:${NC}"
            echo "  all-basic           - All offline tests"
            echo "  all-download        - All download tests"
            echo "  all                 - All tests"
            echo -e "${GREEN}Examples:${NC}"
            echo "  ./test_ytdlp.sh diagnostics"
            echo "  ./test_ytdlp.sh download"
            echo "  ./test_ytdlp.sh all-basic"
            echo -e "${YELLOW}Tip: run 'diagnostics' first to check readiness${NC}"
            exit 0
            ;;
        *)
            echo -e "${RED}❌ Unknown test: $1${NC}"
            echo "Run './test_ytdlp.sh help' for the list"
            exit 1
            ;;
    esac
    exit 0
fi

# Default: full diagnostics
print_banner
echo -e "${YELLOW}💡 No test specified - running full diagnostics${NC}"
echo -e "${YELLOW}   For list: ./test_ytdlp.sh help${NC}\n"
run_test test_full_diagnostics
print_banner
