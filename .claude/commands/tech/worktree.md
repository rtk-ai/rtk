---
model: haiku
description: Git Worktree Setup for RTK
---

# Git Worktree Setup

Create isolated git worktrees with instant feedback and background Cargo check.

**Performance**: ~1s setup + background cargo check

## Usage

```bash
/tech:worktree feature/new-filter     # Creates worktree + background cargo check
/tech:worktree fix/typo --fast        # Skip cargo check (instant)
/tech:worktree feature/perf --no-check  # Skip cargo check
```

**Behavior**: Creates the worktree and displays the path. Navigate manually with `cd .worktrees/{branch-name}`.

**⚠️ Important - Claude Context**: If Claude Code is currently running, restart it in the new worktree:
```bash
/exit                                    # Exit current Claude session
cd .worktrees/fix-bug-name              # Navigate to worktree
claude                                   # Start Claude in worktree context
```

Check cargo check status: `/tech:worktree-status feature/new-filter`

## Branch Naming Convention

**Always use Git branch naming with slashes:**

- ✅ `feature/new-filter` → Branch: `feature/new-filter`, Directory: `.worktrees/feature-new-filter`
- ✅ `fix/bug-name` → Branch: `fix/bug-name`, Directory: `.worktrees/fix-bug-name`
- ❌ `feature-new-filter` → Wrong: Missing category prefix

## Implementation

Execute this **single bash script** with branch name from `$ARGUMENTS`:

```bash
#!/bin/bash
set -euo pipefail

trap 'kill $(jobs -p) 2>/dev/null || true' EXIT

# Validate git repository - always use main repo root (not worktree root)
GIT_COMMON_DIR="$(git rev-parse --git-common-dir 2>/dev/null)"
if [ -z "$GIT_COMMON_DIR" ]; then
  echo "❌ Not in a git repository"
  exit 1
fi
REPO_ROOT="$(cd "$GIT_COMMON_DIR/.." && pwd)"

# Parse flags
RAW_ARGS="$ARGUMENTS"
BRANCH_NAME="$RAW_ARGS"
SKIP_CHECK=false

if [[ "$RAW_ARGS" == *"--fast"* ]]; then
  SKIP_CHECK=true
  BRANCH_NAME="${BRANCH_NAME// --fast/}"
fi
if [[ "$RAW_ARGS" == *"--no-check"* ]]; then
  SKIP_CHECK=true
  BRANCH_NAME="${BRANCH_NAME// --no-check/}"
fi

# Validate branch name
if [[ "$BRANCH_NAME" =~ [[:space:]\$\`] ]]; then
  echo "❌ Invalid branch name (spaces or special characters not allowed)"
  exit 1
fi
if [[ "$BRANCH_NAME" =~ [~^:?*\\\[\]] ]]; then
  echo "❌ Invalid branch name (git forbidden characters: ~ ^ : ? * [ ])"
  exit 1
fi

# Paths - sanitize slashes to avoid nested directories
WORKTREE_NAME="${BRANCH_NAME//\//-}"
WORKTREE_DIR="$REPO_ROOT/.worktrees/$WORKTREE_NAME"
LOG_FILE="/tmp/worktree-cargo-check-${WORKTREE_NAME}.log"

# 1. Check .gitignore (fail-fast)
if ! grep -qE "^\.worktrees/?$" "$REPO_ROOT/.gitignore" 2>/dev/null; then
  echo "❌ .worktrees/ not in .gitignore"
  echo "Run: echo '.worktrees/' >> .gitignore && git add .gitignore && git commit -m 'chore: ignore worktrees'"
  exit 1
fi

# 2. Create worktree (fail-fast)
echo "Creating worktree for $BRANCH_NAME..."
mkdir -p "$REPO_ROOT/.worktrees"
if ! git worktree add "$WORKTREE_DIR" -b "$BRANCH_NAME" 2>/tmp/worktree-error.log; then
  echo "❌ Failed to create worktree"
  cat /tmp/worktree-error.log
  exit 1
fi

# 3. Background cargo check (unless --fast / --no-check)
if [ "$SKIP_CHECK" = false ] && [ -f "$WORKTREE_DIR/Cargo.toml" ]; then
  (
    cd "$WORKTREE_DIR"
    echo "⏳ Cargo check started at $(date +%H:%M:%S)" > "$LOG_FILE"
    if cargo check --all-targets >> "$LOG_FILE" 2>&1; then
      echo "✅ Cargo check passed at $(date +%H:%M:%S)" >> "$LOG_FILE"
    else
      echo "❌ Cargo check failed at $(date +%H:%M:%S)" >> "$LOG_FILE"
    fi
  ) &
  CHECK_RUNNING=true
else
  CHECK_RUNNING=false
fi

# 4. Report (instant feedback)
echo ""
echo "✅ Worktree ready: $WORKTREE_DIR"

if [ "$CHECK_RUNNING" = true ]; then
  echo "⏳ Cargo check running in background..."
  echo "📝 Check status: /tech:worktree-status $BRANCH_NAME"
  echo "📝 Or view log: cat $LOG_FILE"
elif [ "$SKIP_CHECK" = true ]; then
  echo "⚡ Cargo check skipped (--fast / --no-check mode)"
fi

echo ""
echo "🚀 Next steps:"
echo ""
echo "If Claude Code is running:"
echo "   1. /exit"
echo "   2. cd $WORKTREE_DIR"
echo "   3. claude"
echo ""
echo "If Claude Code is NOT running:"
echo "   cd $WORKTREE_DIR && claude"
echo ""
echo "✅ Ready to work!"
```

## Flags

### `--fast` / `--no-check`

Skip cargo check entirely (instant setup).

**Use when**: Quick fixes, documentation, README changes.

```bash
/tech:worktree fix/typo --fast
→ ✅ Ready in 1s (no cargo check)
```

## Status Check

```bash
/tech:worktree-status feature/new-filter
→ ✅ Cargo check passed (0 errors)
→ ❌ Cargo check failed (see log)
→ ⏳ Still running...
```

## Cleanup

```bash
/tech:remove-worktree feature/new-filter
# Or manually:
git worktree remove .worktrees/feature-new-filter
git worktree prune
```

## Troubleshooting

**"worktree already exists"**
```bash
git worktree remove .worktrees/$BRANCH_NAME
# Then retry
```

**"branch already exists"**
```bash
git branch -D $BRANCH_NAME
# Then retry
```
