#!/usr/bin/env bash
# Auto-format Rust code before commits
# Hook: PreToolUse for git commit

echo "🦀 Running Rust pre-commit checks..."

# Format code
cargo fmt --all

# Check for compilation errors only (warnings allowed)
if cargo clippy --all-targets 2>&1 | grep -q "error:"; then
    echo "❌ Clippy found errors. Fix them before committing."
    exit 1
fi

echo "✅ Pre-commit checks passed (warnings allowed)"
