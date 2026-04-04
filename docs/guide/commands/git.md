---
title: Git Commands
description: RTK filters for git and GitHub CLI — compact status, log, diff, and more
sidebar:
  order: 1
---

# Git Commands

RTK supports all git subcommands. Unrecognized subcommands pass through to git unchanged.

## git status — 80% savings

```bash
rtk git status [args...]
```

Replaces the verbose multi-section output with a compact one-line summary.

**Before (20 lines):**
```
On branch main
Your branch is up to date with 'origin/main'.

Changes not staged for commit:
  (use "git add <file>..." to update what will be committed)
        modified:   src/main.rs
        modified:   src/git.rs

Untracked files:
  (use "git add <include>..." to include in what will be committed)
        new_file.txt
...
```

**After (5 lines):**
```
main | 3M 1? 1A
M src/main.rs
M src/git.rs
? new_file.txt
A staged_file.rs
```

## git log — 80% savings

```bash
rtk git log [args...]    # supports --oneline, --graph, --all, -n, etc.
```

Shows hash + subject only, one line per commit.

**Before (50+ lines for 5 commits):**
```
commit abc123def... (HEAD -> main)
Author: User <user@email.com>
Date:   Mon Jan 15 10:30:00 2024

    Fix token counting bug

commit def456...
...
```

**After (5 lines):**
```
abc123 Fix token counting bug
def456 Add vitest support
789abc Refactor filter engine
012def Update README
345ghi Initial commit
```

## git diff — 75% savings

```bash
rtk git diff [args...]    # supports --stat, --cached, --staged, etc.
```

Shows changed files with line counts and condensed hunks.

**Before (~100 lines):**
```
diff --git a/src/main.rs b/src/main.rs
index abc123..def456 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -10,6 +10,8 @@
   fn main() {
+    let config = Config::load()?;
+    config.validate()?;
...30 lines of headers and context...
```

**After (~25 lines):**
```
src/main.rs (+5/-2)
   +  let config = Config::load()?;
   +  config.validate()?;
   -  // old code
   -  let x = 42;
src/git.rs (+1/-1)
   ~  format!("ok {}", branch)
```

## git show — 80% savings

```bash
rtk git show [args...]
```

Shows commit summary + stat + compact diff.

## git add — 92% savings

```bash
rtk git add [args...]    # supports -A, -p, --all, etc.
```

Output: `ok` (single word).

## git commit — 92% savings

```bash
rtk git commit -m "message" [args...]    # supports -a, --amend, --allow-empty, etc.
```

Output: `ok abc1234` (confirmation + short hash).

## git push — 92% savings

```bash
rtk git push [args...]    # supports -u, remote, branch, etc.
```

**Before (15 lines):**
```
Enumerating objects: 5, done.
Counting objects: 100% (5/5), done.
Delta compression using up to 8 threads
...
```

**After (1 line):**
```
ok main
```

## git pull — 92% savings

```bash
rtk git pull [args...]
```

Output: `ok 3 files +10 -2`

## git branch

```bash
rtk git branch [args...]    # supports -d, -D, -m, etc.
```

Shows current branch, local branches, and remote branches in compact form.

## git fetch

```bash
rtk git fetch [args...]
```

Output: `ok fetched (N new refs)`

## git stash

```bash
rtk git stash [list|show|pop|apply|drop|push] [args...]
```

## git worktree

```bash
rtk git worktree [add|remove|prune|list] [args...]
```

## Passthrough

Any git subcommand without a specific RTK filter runs unchanged:

```bash
rtk git rebase main        # runs git rebase main
rtk git cherry-pick abc    # runs git cherry-pick abc
rtk git tag v1.0.0         # runs git tag v1.0.0
```

## GitHub CLI

```bash
rtk gh pr list
rtk gh pr view <num>       # 87% savings
rtk gh pr checks <num>     # 79% savings
rtk gh issue list
rtk gh run list            # 82% savings
rtk gh api <endpoint>
```

**Before (30 lines for pr list):**
```
Showing 10 of 15 pull requests in org/repo

#42  feat: add vitest support
  user opened about 2 days ago
  labels: enhancement
...
```

**After (10 lines):**
```
#42 feat: add vitest (open, 2d)
#41 fix: git diff crash (open, 3d)
#40 chore: update deps (merged, 5d)
```
