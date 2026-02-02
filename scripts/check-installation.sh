#!/bin/bash
# RTK Installation Verification Script
# Helps diagnose if you have the correct rtk (Token Killer) installed

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo "═══════════════════════════════════════════════════════════"
echo "           RTK Installation Verification"
echo "═══════════════════════════════════════════════════════════"
echo ""

# Check 1: RTK installed?
echo "1. Checking if RTK is installed..."
if command -v rtk &> /dev/null; then
    echo -e "   ${GREEN}✅ RTK is installed${NC}"
    RTK_PATH=$(which rtk)
    echo "   Location: $RTK_PATH"
else
    echo -e "   ${RED}❌ RTK is NOT installed${NC}"
    echo ""
    echo "   Install with:"
    echo "   curl -fsSL https://github.com/rtk-ai/rtk/blob/master/install.sh| sh"
    exit 1
fi
echo ""

# Check 2: RTK version
echo "2. Checking RTK version..."
RTK_VERSION=$(rtk --version 2>/dev/null || echo "unknown")
echo "   Version: $RTK_VERSION"
echo ""

# Check 3: Is it Token Killer or Type Kit?
echo "3. Verifying this is Token Killer (not Type Kit)..."
if rtk gain &>/dev/null || rtk gain --help &>/dev/null; then
    echo -e "   ${GREEN}✅ CORRECT - You have Rust Token Killer${NC}"
    CORRECT_RTK=true
else
    echo -e "   ${RED}❌ WRONG - You have Rust Type Kit (different project!)${NC}"
    echo ""
    echo "   You installed the wrong package. Fix it with:"
    echo "   cargo uninstall rtk"
    echo "   curl -fsSL https://github.com/rtk-ai/rtk/blob/master/install.sh | sh"
    CORRECT_RTK=false
fi
echo ""

if [ "$CORRECT_RTK" = false ]; then
    echo "═══════════════════════════════════════════════════════════"
    echo -e "${RED}INSTALLATION CHECK FAILED${NC}"
    echo "═══════════════════════════════════════════════════════════"
    exit 1
fi

# Check 4: Available features
echo "4. Checking available features..."
FEATURES=()
MISSING_FEATURES=()

check_command() {
    local cmd=$1
    local name=$2
    if rtk --help 2>/dev/null | grep -qw "$cmd"; then
        echo -e "   ${GREEN}✅${NC} $name"
        FEATURES+=("$name")
    else
        echo -e "   ${YELLOW}⚠️${NC}  $name (missing - upgrade to fork?)"
        MISSING_FEATURES+=("$name")
    fi
}

check_command "gain" "Token savings analytics"
check_command "git" "Git operations"
check_command "gh" "GitHub CLI"
check_command "pnpm" "pnpm support"
check_command "vitest" "Vitest test runner"
check_command "lint" "ESLint/linters"
check_command "tsc" "TypeScript compiler"
check_command "next" "Next.js"
check_command "prettier" "Prettier"
check_command "playwright" "Playwright E2E"
check_command "prisma" "Prisma ORM"
check_command "discover" "Discover missed savings"

echo ""

# Check 5: CLAUDE.md initialization
echo "5. Checking Claude Code integration..."
GLOBAL_INIT=false
LOCAL_INIT=false

if [ -f "$HOME/.claude/CLAUDE.md" ] && grep -q "rtk" "$HOME/.claude/CLAUDE.md"; then
    echo -e "   ${GREEN}✅${NC} Global CLAUDE.md initialized (~/.claude/CLAUDE.md)"
    GLOBAL_INIT=true
else
    echo -e "   ${YELLOW}⚠️${NC}  Global CLAUDE.md not initialized"
    echo "      Run: rtk init --global"
fi

if [ -f "./CLAUDE.md" ] && grep -q "rtk" "./CLAUDE.md"; then
    echo -e "   ${GREEN}✅${NC} Local CLAUDE.md initialized (./CLAUDE.md)"
    LOCAL_INIT=true
else
    echo -e "   ${YELLOW}⚠️${NC}  Local CLAUDE.md not initialized in current directory"
    echo "      Run: rtk init (in your project directory)"
fi
echo ""

# Check 6: Auto-rewrite hook
echo "6. Checking auto-rewrite hook (optional but recommended)..."
if [ -f "$HOME/.claude/hooks/rtk-rewrite.sh" ]; then
    echo -e "   ${GREEN}✅${NC} Hook script installed"
    if [ -f "$HOME/.claude/settings.json" ] && grep -q "rtk-rewrite.sh" "$HOME/.claude/settings.json"; then
        echo -e "   ${GREEN}✅${NC} Hook enabled in settings.json"
    else
        echo -e "   ${YELLOW}⚠️${NC}  Hook script exists but not enabled in settings.json"
        echo "      See README.md 'Auto-Rewrite Hook' section"
    fi
else
    echo -e "   ${YELLOW}⚠️${NC}  Auto-rewrite hook not installed (optional)"
    echo "      Install: cp .claude/hooks/rtk-rewrite.sh ~/.claude/hooks/"
fi
echo ""

# Summary
echo "═══════════════════════════════════════════════════════════"
echo "                    SUMMARY"
echo "═══════════════════════════════════════════════════════════"

if [ ${#MISSING_FEATURES[@]} -gt 0 ]; then
    echo -e "${YELLOW}⚠️  You have a basic RTK installation${NC}"
    echo ""
    echo "Missing features:"
    for feature in "${MISSING_FEATURES[@]}"; do
        echo "  - $feature"
    done
    echo ""
    echo "To get all features, install the fork:"
    echo "  cargo uninstall rtk"
    echo "  curl -fsSL https://github.com/rtk-ai/rtk/blob/master/install.sh | sh"
    echo "  cd rtk && git checkout feat/all-features"
    echo "  cargo install --path . --force"
else
    echo -e "${GREEN}✅ Full-featured RTK installation detected${NC}"
fi

echo ""

if [ "$GLOBAL_INIT" = false ] && [ "$LOCAL_INIT" = false ]; then
    echo -e "${YELLOW}⚠️  RTK not initialized for Claude Code${NC}"
    echo "   Run: rtk init --global (for all projects)"
    echo "   Or:  rtk init (for this project only)"
fi

echo ""
echo "Need help? See docs/TROUBLESHOOTING.md"
echo "═══════════════════════════════════════════════════════════"
