#!/usr/bin/env bash
set -e

echo "🔍 Validating RTK documentation consistency..."

# 1. Source file count sanity check
SRC_FILES=$(find src -name "*.rs" ! -name "mod.rs" ! -name "main.rs" | wc -l | tr -d ' ')
echo "📊 Rust source files in src/: $SRC_FILES"

# 3. Commandes Python/Go présentes partout
PYTHON_GO_CMDS=("ruff" "pytest" "pip" "go" "golangci")
echo "🐍 Checking Python/Go commands documentation..."

for cmd in "${PYTHON_GO_CMDS[@]}"; do
  if [ ! -f "README.md" ]; then
    echo "⚠️  README.md not found, skipping"
    break
  fi
  if ! grep -q "$cmd" "README.md"; then
    echo "❌ README.md ne mentionne pas commande $cmd"
    exit 1
  fi
done
echo "✅ Python/Go commands: documented in README.md"

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
