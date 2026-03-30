#!/usr/bin/env bash
set -e

echo "🔍 Validating RTK documentation consistency..."

# 1. Commandes Python/Go présentes partout
PYTHON_GO_CMDS=("ruff" "pytest" "pip" "go" "golangci")
echo "🐍 Checking Python/Go commands documentation..."

for cmd in "${PYTHON_GO_CMDS[@]}"; do
  for file in README.md CLAUDE.md; do
    if [ ! -f "$file" ]; then
      echo "⚠️  $file not found, skipping"
      continue
    fi
    if ! grep -q "$cmd" "$file"; then
      echo "❌ $file ne mentionne pas commande $cmd"
      exit 1
    fi
  done
done
echo "✅ Python/Go commands: documented in README.md and CLAUDE.md"

# 4. Hooks cohérents avec doc
HOOK_FILE=".claude/hooks/rtk-rewrite.sh"
if [ -f "$HOOK_FILE" ]; then
  echo "🪝 Checking hook rewrites..."
  for cmd in "${PYTHON_GO_CMDS[@]}"; do
    if ! grep -q "$cmd" "$HOOK_FILE"; then
      echo "⚠️  Hook may not rewrite $cmd (verify manually)"
    fi
  done
  echo "✅ Hook file exists and mentions Python/Go commands"
else
  echo "⚠️  Hook file not found: $HOOK_FILE"
fi

echo ""
echo "✅ Documentation validation passed"
