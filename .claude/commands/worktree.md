---
model: haiku
description: Git Worktree Setup for RTK (Rust project)
---

# Git Worktree Setup

Create isolated git worktrees with instant feedback and background Rust verification.

**Performance**: ~1s setup + background `cargo check` (non-blocking)

## Usage

```bash
/worktree feature/new-filter       # Creates worktree + background cargo check
/worktree fix/typo --fast          # Skip cargo check (instant)
/worktree feature/big-refactor --check  # Wait for cargo check (blocking)
```

**Branch naming**: Always use `category/description` with a slash.

- `feature/new-filter` -> branch: `feature/new-filter`, dir: `.worktrees/feature-new-filter`
- `fix/bug-name` -> branch: `fix/bug-name`, dir: `.worktrees/fix-bug-name`

## Implementation

Execute this **single bash script** with branch name from `$ARGUMENTS`:

```bash
#!/bin/bash
set -euo pipefail

trap 'kill $(jobs -p) 2>/dev/null || true' EXIT

# Resolve main repo root (works from worktree too)
GIT_COMMON_DIR="$(git rev-parse --git-common-dir 2>/dev/null)"
if [ -z "$GIT_COMMON_DIR" ]; then
  echo "Not in a git repository"
  exit 1
fi
REPO_ROOT="$(cd "$GIT_COMMON_DIR/.." && pwd)"

# Parse flags
RAW_ARGS="$ARGUMENTS"
BRANCH_NAME="$RAW_ARGS"
SKIP_CHECK=false
BLOCKING_CHECK=false

if [[ "$RAW_ARGS" == *"--fast"* ]]; then
  SKIP_CHECK=true
  BRANCH_NAME="${BRANCH_NAME// --fast/}"
fi
if [[ "$RAW_ARGS" == *"--check"* ]]; then
  BLOCKING_CHECK=true
  BRANCH_NAME="${BRANCH_NAME// --check/}"
fi

# Validate branch name
if [[ "$BRANCH_NAME" =~ [[:space:]\$\`] ]]; then
  echo "Invalid branch name (spaces or special characters not allowed)"
  exit 1
fi
if [[ "$BRANCH_NAME" =~ [~^:?*\\\[\]] ]]; then
  echo "Invalid branch name (git forbidden characters)"
  exit 1
fi

# Paths
WORKTREE_NAME="${BRANCH_NAME//\//-}"
WORKTREE_DIR="$REPO_ROOT/.worktrees/$WORKTREE_NAME"
LOG_FILE="/tmp/worktree-cargocheck-${WORKTREE_NAME}.log"

# 1. Check .gitignore (fail-fast)
if ! grep -qE "^\.worktrees/?$" "$REPO_ROOT/.gitignore" 2>/dev/null; then
  echo ".worktrees/ not in .gitignore"
  echo "Run: echo '.worktrees/' >> .gitignore && git add .gitignore && git commit -m 'chore: ignore worktrees'"
  exit 1
fi

# 2. Create worktree
echo "Creating worktree for $BRANCH_NAME..."
mkdir -p "$REPO_ROOT/.worktrees"
if ! git worktree add "$WORKTREE_DIR" -b "$BRANCH_NAME" 2>/tmp/worktree-error.log; then
  echo "Failed to create worktree:"
  cat /tmp/worktree-error.log
  exit 1
fi

# 3. Copy files listed in .worktreeinclude (non-blocking)
(
  INCLUDE_FILE="$REPO_ROOT/.worktreeinclude"
  if [ -f "$INCLUDE_FILE" ]; then
    while IFS= read -r entry || [ -n "$entry" ]; do
      [[ "$entry" =~ ^#.*$ || -z "$entry" ]] && continue
      entry="$(echo "$entry" | xargs)"
      SRC="$REPO_ROOT/$entry"
      if [ -e "$SRC" ]; then
        DEST_DIR="$(dirname "$WORKTREE_DIR/$entry")"
        mkdir -p "$DEST_DIR"
        cp -R "$SRC" "$WORKTREE_DIR/$entry"
      fi
    done < "$INCLUDE_FILE"
  else
    cp "$REPO_ROOT"/.env* "$WORKTREE_DIR/" 2>/dev/null || true
  fi
) &
ENV_PID=$!

# Wait for env copy (with macOS-compatible timeout)
# gtimeout from coreutils if available, else plain wait
if command -v gtimeout >/dev/null 2>&1; then
  gtimeout 10 wait $ENV_PID 2>/dev/null || true
else
  wait $ENV_PID 2>/dev/null || true
fi

# 4. cargo check (background by default, blocking with --check)
if [ "$SKIP_CHECK" = false ]; then
  if [ "$BLOCKING_CHECK" = true ]; then
    echo "Running cargo check..."
    if (cd "$WORKTREE_DIR" && cargo check 2>&1); then
      echo "cargo check passed"
    else
      echo "cargo check failed (worktree still usable)"
    fi
    CHECK_RUNNING=false
  else
    # Background
    (
      cd "$WORKTREE_DIR"
      echo "cargo check started at $(date +%H:%M:%S)" > "$LOG_FILE"
      if cargo check >> "$LOG_FILE" 2>&1; then
        echo "PASSED at $(date +%H:%M:%S)" >> "$LOG_FILE"
      else
        echo "FAILED at $(date +%H:%M:%S)" >> "$LOG_FILE"
      fi
    ) &
    CHECK_RUNNING=true
  fi
else
  CHECK_RUNNING=false
fi

# 5. Report
echo ""
echo "Worktree ready: $WORKTREE_DIR"
echo "Branch: $BRANCH_NAME"

if [ "$CHECK_RUNNING" = true ]; then
  echo "cargo check running in background..."
  echo "Check status: /worktree-status $BRANCH_NAME"
  echo "Or view log: cat $LOG_FILE"
elif [ "$SKIP_CHECK" = true ]; then
  echo "cargo check skipped (--fast)"
fi

echo ""
echo "Next steps:"
echo ""
echo "If Claude Code is running:"
echo "   1. /exit"
echo "   2. cd $WORKTREE_DIR"
echo "   3. claude"
echo ""
echo "If Claude Code is NOT running:"
echo "   cd $WORKTREE_DIR && claude"
```

## Flags

### `--fast`
Skip `cargo check` (instant setup). Use for quick fixes, docs, small changes.

### `--check`
Run `cargo check` synchronously (blocking). Use when you need to confirm the build is clean before starting.

## Environment Files

Files listed in `.worktreeinclude` are copied automatically. If the file doesn't exist, `.env*` files are copied by default.

Example `.worktreeinclude` for RTK:
```
.env
.env.local
.claude/settings.local.json
```

## Cleanup

```bash
git worktree remove .worktrees/${BRANCH_NAME//\//-}
git worktree prune
```

## Troubleshooting

**"worktree already exists"**
```bash
git worktree remove .worktrees/feature-name
```

**"branch already exists"**
```bash
git branch -D feature/name
```

**cargo check log not found**
```bash
ls /tmp/worktree-cargocheck-*.log
```
