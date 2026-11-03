---
model: haiku
description: Check background cargo check status for a git worktree
argument-hint: "<branch-name>"
---

# Worktree Status Check

Check the status of the background `cargo check` started by `/worktree`.

## Usage

```bash
/worktree-status feature/new-filter
/worktree-status fix/bug-name
```

## Implementation

Execute this script with branch name from `$ARGUMENTS`:

```bash
#!/bin/bash
set -euo pipefail

BRANCH_NAME="$ARGUMENTS"
LOG_FILE="/tmp/worktree-cargocheck-${BRANCH_NAME//\//-}.log"

if [ ! -f "$LOG_FILE" ]; then
  echo "No cargo check found for branch: $BRANCH_NAME"
  echo ""
  echo "Possible reasons:"
  echo "1. Worktree created with --fast (check skipped)"
  echo "2. Branch name mismatch (use exact branch name)"
  echo "3. Check hasn't started yet (wait a few seconds)"
  echo ""
  echo "Available logs:"
  ls -1 /tmp/worktree-cargocheck-*.log 2>/dev/null || echo "  (none)"
  exit 1
fi

LOG_CONTENT=$(head -n 500 "$LOG_FILE")

if echo "$LOG_CONTENT" | grep -q "^PASSED"; then
  TIMESTAMP=$(echo "$LOG_CONTENT" | grep "^PASSED" | sed 's/PASSED at //')
  echo "cargo check passed"
  echo "   Completed at: $TIMESTAMP"
  echo ""
  echo "Worktree is ready for development!"

elif echo "$LOG_CONTENT" | grep -q "^FAILED"; then
  TIMESTAMP=$(echo "$LOG_CONTENT" | grep "^FAILED" | sed 's/FAILED at //')
  echo "cargo check failed"
  echo "   Completed at: $TIMESTAMP"
  echo ""
  echo "Errors:"
  echo "-------------------------------------"
  grep -v "^PASSED\|^FAILED\|^cargo check started" "$LOG_FILE" | head -30
  echo "-------------------------------------"
  echo ""
  echo "Full log: cat $LOG_FILE"
  echo ""
  echo "You can still work on the worktree - fix errors as you go."

elif echo "$LOG_CONTENT" | grep -q "^cargo check started"; then
  START_TIME=$(echo "$LOG_CONTENT" | grep "^cargo check started" | sed 's/cargo check started at //')
  CURRENT_TIME=$(date +%H:%M:%S)
  echo "cargo check still running..."
  echo "   Started at: $START_TIME"
  echo "   Current time: $CURRENT_TIME"
  echo ""
  echo "Usually takes 5-30s depending on crate size."
  echo ""
  echo "Live progress: tail -f $LOG_FILE"

else
  echo "Unknown state"
  echo ""
  echo "Log content:"
  cat "$LOG_FILE"
fi
```

## Output Examples

### Passed
```
cargo check passed
   Completed at: 14:23:45

Worktree is ready for development!
```

### Failed
```
cargo check failed
   Completed at: 14:24:12

Errors:
-------------------------------------
error[E0308]: mismatched types
  --> src/git.rs:45:12
   |
45 |     let x: i32 = "hello";
-------------------------------------

Full log: cat /tmp/worktree-cargocheck-feature-new-filter.log

You can still work on the worktree - fix errors as you go.
```

### Still Running
```
cargo check still running...
   Started at: 14:22:30
   Current time: 14:22:45

Usually takes 5-30s depending on crate size.

Live progress: tail -f /tmp/worktree-cargocheck-feature-new-filter.log
```

## Integration

`/worktree` tells you the exact command to check status:
```
cargo check running in background...
Check status: /worktree-status feature/new-filter
```
