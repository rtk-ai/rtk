#!/usr/bin/env bash
# RTK OpenCode integration setup script
#
# Usage:
#   ./setup.sh              # Install globally (~/.config/opencode/)
#   ./setup.sh --local      # Install into current project (.opencode/)
#
# Requires: rtk >= 0.23.0 on PATH
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# ── Flags ──────────────────────────────────────────────────────────
LOCAL=false
for arg in "$@"; do
  case "$arg" in
    --local) LOCAL=true ;;
    --help|-h)
      echo "Usage: $0 [--local]"
      echo "  --local  Install into .opencode/ in the current directory"
      echo "  (default) Install globally into ~/.config/opencode/"
      exit 0
      ;;
  esac
done

# ── Pre-flight ─────────────────────────────────────────────────────
if ! command -v rtk &>/dev/null; then
  echo -e "${RED}Error: rtk not found on PATH${NC}"
  echo "Install: cargo install --git https://github.com/rtk-ai/rtk"
  exit 1
fi

RTK_VERSION=$(rtk --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)
if [ -n "$RTK_VERSION" ]; then
  MAJOR=$(echo "$RTK_VERSION" | cut -d. -f1)
  MINOR=$(echo "$RTK_VERSION" | cut -d. -f2)
  if [ "$MAJOR" -eq 0 ] && [ "$MINOR" -lt 23 ]; then
    echo -e "${RED}Error: rtk $RTK_VERSION is too old (need >= 0.23.0)${NC}"
    echo "Upgrade: cargo install --git https://github.com/rtk-ai/rtk --force"
    exit 1
  fi
fi

# ── Determine target directories ───────────────────────────────────
if [ "$LOCAL" = true ]; then
  PLUGIN_DIR=".opencode/plugins"
  AGENTS_MD="AGENTS.md"
  SCOPE="project"
else
  PLUGIN_DIR="$HOME/.config/opencode/plugins"
  AGENTS_MD="$HOME/.config/opencode/AGENTS.md"
  SCOPE="global"
fi

# ── Install plugin ─────────────────────────────────────────────────
mkdir -p "$PLUGIN_DIR"
cp "$SCRIPT_DIR/plugins/rtk-rewrite.ts" "$PLUGIN_DIR/rtk-rewrite.ts"
echo -e "${GREEN}✅ Plugin installed: $PLUGIN_DIR/rtk-rewrite.ts${NC}"

# ── Patch AGENTS.md with RTK awareness ─────────────────────────────
RTK_AWARENESS="$SCRIPT_DIR/rtk-awareness.md"

if [ -f "$AGENTS_MD" ]; then
  if grep -q "RTK - Rust Token Killer" "$AGENTS_MD" 2>/dev/null; then
    echo -e "${YELLOW}⚠️  AGENTS.md already contains RTK instructions — skipping${NC}"
  else
    echo "" >> "$AGENTS_MD"
    cat "$RTK_AWARENESS" >> "$AGENTS_MD"
    echo -e "${GREEN}✅ RTK awareness appended to $AGENTS_MD${NC}"
  fi
else
  cp "$RTK_AWARENESS" "$AGENTS_MD"
  echo -e "${GREEN}✅ Created $AGENTS_MD with RTK instructions${NC}"
fi

# ── Done ───────────────────────────────────────────────────────────
echo ""
echo -e "${GREEN}RTK + OpenCode integration installed (${SCOPE}).${NC}"
echo ""
echo "  Next steps:"
echo "    1. Restart OpenCode"
echo "    2. Run a command — e.g. git status — and verify rtk is used"
echo "    3. Check savings: rtk gain"
echo ""
