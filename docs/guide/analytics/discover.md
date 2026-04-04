---
title: Discover Missed Savings
description: Find commands that ran without RTK and could have saved tokens
sidebar:
  order: 2
---

# Discover Missed Savings

`rtk discover` analyzes Claude Code's command history to find commands that ran without RTK and could have been optimized.

## Usage

```bash
rtk discover                          # current project, last 30 days
rtk discover --all --since 7          # all projects, last 7 days
rtk discover -p /path/to/project      # filter by project path
rtk discover --limit 20               # max commands per section
rtk discover --format json            # JSON export
```

## Options

| Option | Short | Description |
|--------|-------|-------------|
| `--project` | `-p` | Filter by project path |
| `--limit` | `-l` | Max commands per section (default: 15) |
| `--all` | `-a` | Scan all projects |
| `--since` | `-s` | Last N days (default: 30) |
| `--format` | `-f` | Output format: `text`, `json` |

## Example output

```
RTK Missed Opportunities (last 30 days)

Commands that could have used RTK:
  git log --oneline -20   ×12   (est. 80% savings each)
  cargo test              ×8    (est. 90% savings each)
  pnpm list               ×5    (est. 70% savings each)

Estimated savings missed: ~340K tokens
```

## How it works

RTK reads Claude Code's command history database (the same one that backs `rtk gain --history`). It matches raw commands against the RTK rewrite registry and flags instances where RTK was not used but a filter exists.

Use this after setting up RTK to see how much you were leaving on the table before.
