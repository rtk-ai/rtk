---
title: GitHub CLI
description: RTK filters for gh — pull requests, issues, checks, and workflow runs
sidebar:
  order: 10
---

# GitHub CLI

RTK filters `gh` output by removing ASCII art, verbose metadata, and decorative formatting, keeping the information that matters.

## pr list — 80% savings

```bash
rtk gh pr list [args...]
```

**Before (~30 lines):**
```
Showing 3 of 3 pull requests in org/repo

#42  feat: add vitest support
  user1 opened about 2 days ago
  Labels: enhancement
  Review: 0 approving, 0 requesting changes

#41  fix: git diff crash
  user2 opened about 3 days ago
  ...
```

**After (~6 lines):**
```
#42 feat: add vitest (open, 2d)
#41 fix: git diff crash (open, 3d)
#40 chore: update deps (merged, 5d)
```

## pr view — 87% savings

```bash
rtk gh pr view <num> [args...]
```

Compact PR summary: title, status, author, description excerpt, and CI checks in one block.

## pr checks — 79% savings

```bash
rtk gh pr checks <num> [args...]
```

Shows check name + status only, strips URLs and timestamps.

## issue list — 80% savings

```bash
rtk gh issue list [args...]
```

Same compact format as pr list.

## run list — 82% savings

```bash
rtk gh run list [args...]
```

Workflow run name + status + duration, one line each.

## api

```bash
rtk gh api <endpoint> [args...]
```

~26% savings — strips HTTP headers and formats JSON output.

## Stacked PRs (Graphite)

```bash
rtk gt log       # compact stack log
rtk gt submit    # compact submit output
rtk gt sync      # compact sync
rtk gt restack   # compact restack
rtk gt create    # compact create
rtk gt branch    # compact branch info
```

Unrecognized `gt` subcommands pass through to Graphite or git.

## Passthrough

Other `gh` subcommands pass through to the GitHub CLI unchanged.
