---
model: haiku
description: Interactive cleanup of stale worktrees (merged branches, orphaned refs)
---

# Clean Worktree (Interactive)

Interactive cleanup of worktrees: lists merged/stale branches and asks confirmation before deleting.

**Difference with `/clean-worktrees`**:
- `/clean-worktree`: Interactive, asks confirmation
- `/clean-worktrees`: Automatic, no interaction

## Usage

```bash
/clean-worktree    # Interactive audit + cleanup
```

## Implementation

Execute this script:

```bash
#!/bin/bash
set -euo pipefail

echo "=== Worktrees Status ==="
git worktree list
echo ""

echo "=== Pruning stale references ==="
git worktree prune
echo ""

echo "=== Merged branches (safe to delete) ==="
MERGED_FOUND=false
CURRENT_DIR="$(pwd)"

while IFS= read -r line; do
  path=$(echo "$line" | awk '{print $1}')
  branch=$(echo "$line" | grep -oE '\[.*\]' | tr -d '[]' || true)
  [ -z "$branch" ] && continue
  [ "$branch" = "master" ] && continue
  [ "$branch" = "main" ] && continue
  [ "$path" = "$CURRENT_DIR" ] && continue

  if git branch --merged master | grep -q "^[* ] ${branch}$" 2>/dev/null; then
    echo "  - $branch (at $path) - MERGED"
    MERGED_FOUND=true
  fi
done < <(git worktree list)

if [ "$MERGED_FOUND" = false ]; then
  echo "  (none found)"
  echo ""
  echo "=== Disk usage ==="
  du -sh .worktrees/ 2>/dev/null || echo "No .worktrees directory"
  exit 0
fi
echo ""

echo "=== Clean merged worktrees? [y/N] ==="
read -r confirm
if [ "$confirm" = "y" ] || [ "$confirm" = "Y" ]; then
  while IFS= read -r line; do
    path=$(echo "$line" | awk '{print $1}')
    branch=$(echo "$line" | grep -oE '\[.*\]' | tr -d '[]' || true)
    [ -z "$branch" ] && continue
    [ "$branch" = "master" ] && continue
    [ "$branch" = "main" ] && continue
    [ "$path" = "$CURRENT_DIR" ] && continue

    if git branch --merged master | grep -q "^[* ] ${branch}$" 2>/dev/null; then
      echo "  Removing $branch..."
      git worktree remove "$path" 2>/dev/null || rm -rf "$path"
      git branch -d "$branch" 2>/dev/null || echo "    (branch already deleted)"
      echo "  Done: $branch"
    fi
  done < <(git worktree list)
  echo ""
  echo "Cleanup complete."
else
  echo "Aborted."
fi

echo ""
echo "=== Disk usage ==="
du -sh .worktrees/ 2>/dev/null || echo "No .worktrees directory"
```

## Safety

- Never removes `master` or `main` worktrees
- Only removes branches merged into `master`
- Asks confirmation before any deletion
- Cleans both git reference and physical directory

## Manual Force Remove (unmerged branch)

```bash
git worktree remove --force .worktrees/feature-name
git branch -D feature/name
git worktree prune
```
