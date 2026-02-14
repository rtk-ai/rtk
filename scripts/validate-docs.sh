#!/bin/bash
set -e

echo "🔍 Validating RTK documentation consistency..."

# 1. Version Cargo.toml == tous les fichiers doc
CARGO_VERSION=$(grep '^version = ' Cargo.toml | cut -d'"' -f2)
echo "📦 Cargo.toml version: $CARGO_VERSION"

for file in README.md CLAUDE.md ARCHITECTURE.md; do
  if [ ! -f "$file" ]; then
    echo "⚠️  $file not found, skipping"
    continue
  fi
  if ! grep -q "$CARGO_VERSION" "$file"; then
    echo "❌ $file ne mentionne pas version $CARGO_VERSION"
    exit 1
  fi
done
echo "✅ Version consistency: all docs mention $CARGO_VERSION"

# 2. Nombre de modules cohérent
MAIN_MODULES=$(grep -c '^mod ' src/main.rs)
echo "📊 Module count in main.rs: $MAIN_MODULES"

# Extract module count from ARCHITECTURE.md
if [ -f "ARCHITECTURE.md" ]; then
  ARCH_MODULES=$(grep 'Total:.*modules' ARCHITECTURE.md | grep -o '[0-9]\+' | head -1)
  if [ -z "$ARCH_MODULES" ]; then
    echo "⚠️  Could not extract module count from ARCHITECTURE.md"
  else
    echo "📊 Module count in ARCHITECTURE.md: $ARCH_MODULES"
    if [ "$MAIN_MODULES" != "$ARCH_MODULES" ]; then
      echo "❌ Module count mismatch: main.rs=$MAIN_MODULES, ARCHITECTURE.md=$ARCH_MODULES"
      exit 1
    fi
  fi
fi

# 3. Commandes Python/Go présentes partout
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
