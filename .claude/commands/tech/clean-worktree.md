---
model: haiku
description: Clean stale worktrees (interactive)
---

# Clean Worktree (Interactive)

Audit and clean obsolete worktrees interactively: merged, pruned, orphaned branches.

**vs `/tech:clean-worktrees`**:
- `/tech:clean-worktree`: Interactive, asks confirmation before deletion
- `/tech:clean-worktrees`: Automatic, no interaction (merged branches only)

## Usage

```bash
/tech:clean-worktree
```

## Implementation

```bash
#!/bin/bash

echo "=== Worktrees Status ==="
git worktree list
echo ""

echo "=== Pruning stale references ==="
git worktree prune
echo ""

echo "=== Merged branches (safe to delete) ==="
while IFS= read -r line; do
    path=$(echo "$line" | awk '{print $1}')
    branch=$(echo "$line" | grep -oE '\[.*\]' | tr -d '[]')
    [ -z "$branch" ] && continue
    [ "$branch" = "master" ] && continue
    [ "$branch" = "main" ] && continue

    if git branch --merged master | grep -q "^[* ] ${branch}$"; then
        echo "  - $branch (at $path) — MERGED"
    fi
done < <(git worktree list)
echo ""

echo "=== Clean merged worktrees? [y/N] ==="
read -r confirm
if [ "$confirm" = "y" ] || [ "$confirm" = "Y" ]; then
    while IFS= read -r line; do
        path=$(echo "$line" | awk '{print $1}')
        branch=$(echo "$line" | grep -oE '\[.*\]' | tr -d '[]')
        [ -z "$branch" ] && continue
        [ "$branch" = "master" ] && continue
        [ "$branch" = "main" ] && continue

        if git branch --merged master | grep -q "^[* ] ${branch}$"; then
            echo "  Removing $branch..."
            git worktree remove "$path" 2>/dev/null || rm -rf "$path"
            git branch -d "$branch" 2>/dev/null || echo "    (branch already deleted)"
        fi
    done < <(git worktree list)
    echo "Done."
else
    echo "Aborted."
fi

echo ""
echo "=== Disk usage ==="
du -sh .worktrees/ 2>/dev/null || echo "No .worktrees directory"
```

## Safety

- **Never** removes `master` or `main` worktrees
- **Only** removes merged branches (safe)
- **Asks confirmation** before deletion
- Cleans both worktree reference AND physical directory

## Manual Override

Force remove an unmerged worktree:

```bash
git worktree remove --force <path>
git branch -D <branch_name>
```
