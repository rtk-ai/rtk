---
model: haiku
description: Clean all merged worktrees automatically (no interaction)
---

# Clean Worktrees (Automatic)

Automatically remove all worktrees for branches merged into `master`. No interaction required.

**Difference with `/clean-worktree`**:
- `/clean-worktree`: Interactive, asks confirmation per branch
- `/clean-worktrees`: Automatic, removes all merged branches at once

## Usage

```bash
/clean-worktrees              # Remove all merged worktrees
/clean-worktrees --dry-run    # Preview what would be deleted
```

## Implementation

Execute this script:

```bash
#!/bin/bash
set -euo pipefail

DRY_RUN=false
if [[ "${ARGUMENTS:-}" == *"--dry-run"* ]]; then
  DRY_RUN=true
fi

echo "Cleaning Worktrees"
echo "=================="
echo ""

# Step 1: Prune stale git references
echo "1. Pruning stale git references..."
PRUNED=$(git worktree prune -v 2>&1)
if [ -n "$PRUNED" ]; then
  echo "$PRUNED"
  echo "Stale references pruned"
else
  echo "No stale references found"
fi
echo ""

# Step 2: Find merged worktrees
echo "2. Finding merged worktrees..."
MERGED_COUNT=0
MERGED_BRANCHES=()
CURRENT_DIR="$(pwd)"

while IFS= read -r line; do
  path=$(echo "$line" | awk '{print $1}')
  branch=$(echo "$line" | grep -oE '\[.*\]' | tr -d '[]' || true)

  [ -z "$branch" ] && continue
  [ "$branch" = "master" ] && continue
  [ "$branch" = "main" ] && continue
  [ "$path" = "$CURRENT_DIR" ] && continue

  if git branch --merged master | grep -q "^[* ] ${branch}$" 2>/dev/null; then
    MERGED_COUNT=$((MERGED_COUNT + 1))
    MERGED_BRANCHES+=("$branch|$path")
    echo "  - $branch (merged)"
  fi
done < <(git worktree list)

if [ $MERGED_COUNT -eq 0 ]; then
  echo "No merged worktrees found"
  echo ""
  echo "Current worktrees:"
  git worktree list
  exit 0
fi

echo ""
echo "Found $MERGED_COUNT merged worktree(s)"
echo ""

if [ "$DRY_RUN" = true ]; then
  echo "DRY RUN - No changes will be made"
  echo ""
  echo "Would delete:"
  for item in "${MERGED_BRANCHES[@]}"; do
    branch=$(echo "$item" | cut -d'|' -f1)
    path=$(echo "$item" | cut -d'|' -f2)
    echo "  - $branch"
    echo "    Path: $path"
  done
  echo ""
  echo "Run without --dry-run to actually delete"
  exit 0
fi

# Step 3: Remove merged worktrees
echo "3. Removing merged worktrees..."
REMOVED_COUNT=0

for item in "${MERGED_BRANCHES[@]}"; do
  branch=$(echo "$item" | cut -d'|' -f1)
  path=$(echo "$item" | cut -d'|' -f2)

  echo ""
  echo "Removing: $branch"

  if git worktree remove "$path" 2>/dev/null; then
    echo "  Worktree removed"
  else
    echo "  Git remove failed, forcing..."
    rm -rf "$path" 2>/dev/null || true
    git worktree prune 2>/dev/null || true
    echo "  Worktree forcefully removed"
  fi

  if git branch -d "$branch" 2>/dev/null; then
    echo "  Local branch deleted"
  else
    echo "  Local branch already deleted"
  fi

  if git ls-remote --heads origin "$branch" 2>/dev/null | grep -q "$branch"; then
    echo "  Remote branch exists: origin/$branch (not auto-deleted)"
  fi

  REMOVED_COUNT=$((REMOVED_COUNT + 1))
done

echo ""
echo "Cleanup complete"
echo ""
echo "Summary:"
echo "  Removed: $REMOVED_COUNT worktree(s)"
echo ""
echo "Remaining worktrees:"
git worktree list
echo ""

WORKTREES_SIZE=$(du -sh .worktrees/ 2>/dev/null | awk '{print $1}' || echo "N/A")
echo "Worktrees disk usage: $WORKTREES_SIZE"
```

## Safety Features

- Only removes branches merged into `master`
- Skips `master` and `main` (protected)
- Never removes the current working directory
- Dry-run mode to preview before deletion
- Remote branches: reported but not auto-deleted

## When to Use

- After merging PRs: `/clean-worktrees`
- Weekly maintenance: `/clean-worktrees`
- Before creating new worktrees: `/clean-worktrees --dry-run` first

## Manual Removal (unmerged branch)

```bash
git worktree remove --force .worktrees/feature-name
git branch -D feature/name
git worktree prune
```
