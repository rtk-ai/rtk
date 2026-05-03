---
model: haiku
description: Worktree Cargo Check Status
argument-hint: "<branch-name>"
---

# Worktree Status Check

Check the status of background cargo check for a git worktree.

## Usage

```bash
/tech:worktree-status feature/new-filter
/tech:worktree-status fix/session-bug
```

## Implementation

Execute this script with branch name from `$ARGUMENTS`:

```bash
#!/bin/bash
set -euo pipefail

BRANCH_NAME="$ARGUMENTS"
LOG_FILE="/tmp/worktree-cargo-check-${BRANCH_NAME//\//-}.log"

if [ ! -f "$LOG_FILE" ]; then
  echo "❌ No cargo check found for branch: $BRANCH_NAME"
  echo ""
  echo "Possible reasons:"
  echo "1. Worktree was created with --fast / --no-check flag"
  echo "2. Branch name mismatch (use exact branch name)"
  echo "3. Cargo check hasn't started yet (wait a few seconds)"
  echo ""
  echo "Available logs:"
  ls -1 /tmp/worktree-cargo-check-*.log 2>/dev/null || echo "  (none)"
  exit 1
fi

LOG_CONTENT=$(head -n 1000 "$LOG_FILE")

if echo "$LOG_CONTENT" | grep -q "✅ Cargo check passed"; then
  TIMESTAMP=$(echo "$LOG_CONTENT" | grep "Cargo check passed" | sed 's/.*at //')
  echo "✅ Cargo check passed"
  echo "   Completed at: $TIMESTAMP"
  echo ""
  echo "Worktree is ready for development!"

elif echo "$LOG_CONTENT" | grep -q "❌ Cargo check failed"; then
  TIMESTAMP=$(echo "$LOG_CONTENT" | grep "Cargo check failed" | sed 's/.*at //')
  echo "❌ Cargo check failed"
  echo "   Completed at: $TIMESTAMP"
  echo ""
  ERROR_COUNT=$(grep -v "Cargo check" "$LOG_FILE" | grep -c "^error" || echo "0")
  echo "Errors:"
  echo "─────────────────────────────────────"
  grep "^error" "$LOG_FILE" | head -20
  echo "─────────────────────────────────────"
  echo ""
  echo "Full log: cat $LOG_FILE"
  echo ""
  echo "⚠️  You can still work on the worktree - fix errors as you go."

elif echo "$LOG_CONTENT" | grep -q "⏳ Cargo check started"; then
  START_TIME=$(echo "$LOG_CONTENT" | grep "Cargo check started" | sed 's/.*at //')
  CURRENT_TIME=$(date +%H:%M:%S)
  echo "⏳ Cargo check still running..."
  echo "   Started at: $START_TIME"
  echo "   Current time: $CURRENT_TIME"
  echo ""
  echo "Check again in a few seconds or view live progress:"
  echo "  tail -f $LOG_FILE"

else
  echo "⚠️  Cargo check in unknown state"
  echo ""
  echo "Log content:"
  cat "$LOG_FILE"
fi
```

## Output Examples

### Success
```
✅ Cargo check passed
   Completed at: 14:23:45

Worktree is ready for development!
```

### Failed
```
❌ Cargo check failed
   Completed at: 14:24:12

Errors:
─────────────────────────────────────
error[E0308]: mismatched types
  --> src/git.rs:45:12
─────────────────────────────────────

Full log: cat /tmp/worktree-cargo-check-feature-new-filter.log
```

### Still Running
```
⏳ Cargo check still running...
   Started at: 14:22:30
   Current time: 14:22:45

Check again in a few seconds or view live progress:
  tail -f /tmp/worktree-cargo-check-feature-new-filter.log
```
