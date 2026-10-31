---
title: Directives
description: Text RTK puts in front of the agent — shipped directives, your own, how they land in the instruction file and what they cost
sidebar:
  order: 5
---

# Directives

A directive is text RTK writes in front of the agent. It is how RTK reaches what no transform can touch: how the agent writes, which commands it uses.

```toml
[[directive]]
use = "terse-output"

[[directive]]
name = "house-rules"
file = ".rtk/directives/house-rules.md"

[[directive]]
use   = "rtk-awareness"
level = "default"
```

| Key | Notes |
|---|---|
| `use` / `text` / `file` | exactly one: a shipped directive, inline text, or a file relative to the pipeline file |
| `name` | defaults to `use` |
| `channel` | `awareness_file` (default): RTK's instruction file, present every turn |
| `enabled` | |
| module options | `level` for `rtk-awareness` |

## Shipped

| Directive | Ships | What it does |
|---|---|---|
| `rtk-awareness` | on | the awareness levels (`default` · `high` · `full`); `[awareness]` in `config.toml` still applies |
| `terse-output` | off | terse prose; code, paths, commands and errors are never abbreviated, and destructive-action warnings stay full sentences |
| `route-tasks` | generated | written whenever a task has an upstream: tells the agent the exact `rtk <task>` syntax |

A file containing nothing but a directive is a valid pipeline and leaves `rtk:default` intact — see [`rtk:terse`](./reference-pipelines.md#rtkterse--terse-prose).

## How they land

All enabled directives are rendered and joined in declaration order into one block, written once between RTK's markers in the agent's instruction file. Re-running `rtk init` rewrites that block and nothing else.

## What they cost

A directive sits in context every turn. `rtk gain` shows directives as *net* — output tokens saved minus the standing cost.

## Trust

A directive changes the agent's behaviour. `rtk pipeline show` prints every directive verbatim, and `rtk init` shows a project's directives in full before asking to trust the file.
