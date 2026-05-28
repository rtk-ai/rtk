#!/usr/bin/env bash
# Install rtk Copilot hooks in all Git projects under a directory

set -euo pipefail

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Default search directory
SEARCH_DIR="${1:-$HOME/Documents}"

# Counters
TOTAL=0
INSTALLED=0
SKIPPED=0
FAILED=0

echo -e "${BLUE}🔍 Searching for Git projects in: ${SEARCH_DIR}${NC}"
echo ""

# Check if rtk is installed
if ! command -v rtk &> /dev/null; then
    echo -e "${RED}❌ rtk is not installed. Please install it first:${NC}"
    echo "   cargo install --path ."
    exit 1
fi

# Find all .git directories (projects)
while IFS= read -r git_dir; do
    PROJECT_DIR=$(dirname "$git_dir")
    PROJECT_NAME=$(basename "$PROJECT_DIR")
    TOTAL=$((TOTAL + 1))
    
    # Skip if already has rtk hooks
    if [[ -f "$PROJECT_DIR/.github/hooks/rtk-rewrite.json" ]]; then
        echo -e "${YELLOW}⏭️  Skipped${NC} $PROJECT_NAME (already has rtk hooks)"
        SKIPPED=$((SKIPPED + 1))
        continue
    fi
    
    # Try to install
    echo -e "${BLUE}📦 Installing${NC} $PROJECT_NAME"
    if (cd "$PROJECT_DIR" && rtk init --copilot --no-patch > /dev/null 2>&1); then
        echo -e "${GREEN}✅ Installed${NC} $PROJECT_NAME"
        INSTALLED=$((INSTALLED + 1))
    else
        echo -e "${RED}❌ Failed${NC} $PROJECT_NAME"
        FAILED=$((FAILED + 1))
    fi
done < <(find "$SEARCH_DIR" -type d -name ".git" 2>/dev/null)

# Summary
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo -e "${BLUE}📊 Summary${NC}"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo -e "Total projects found:    ${TOTAL}"
echo -e "${GREEN}Installed:${NC}               ${INSTALLED}"
echo -e "${YELLOW}Skipped (already has):${NC}   ${SKIPPED}"
echo -e "${RED}Failed:${NC}                  ${FAILED}"
echo ""

if [[ $INSTALLED -gt 0 ]]; then
    echo -e "${GREEN}✨ rtk successfully installed in ${INSTALLED} project(s)!${NC}"
    echo ""
    echo -e "${BLUE}Next steps:${NC}"
    echo "  1. Restart VS Code to load the new hooks"
    echo "  2. Use GitHub Copilot chat - commands will be auto-rewritten with rtk"
    echo "  3. Run 'rtk gain' to see your token savings"
fi
