---
title: Quick Start
description: Get RTK running in 5 minutes and see your first token savings
sidebar:
  order: 2
---

# Quick Start

This guide walks you through your first RTK commands after installation.

## Prerequisites

RTK is installed and verified:

```bash
rtk --version   # rtk x.y.z
rtk gain        # shows token savings dashboard
```

If not, see [Installation](./installation.md).

## Step 1: Initialize for your AI assistant

```bash
# For Claude Code (global — applies to all projects)
rtk init --global

# For a single project only
cd /your/project && rtk init
```

This installs the hook that automatically rewrites commands. Restart your AI assistant after this step.

## Step 2: Run your first RTK commands

You can use RTK directly or let the hook rewrite commands transparently.

```bash
# Git — compact status and log
rtk git status
rtk git log -10

# Rust — build and test with failures only
rtk cargo build
rtk cargo test

# JavaScript — type errors grouped by file
rtk tsc
rtk vitest run
```

## Step 3: Check your savings

After a few commands, see how much you saved:

```bash
rtk gain
```

Output:

```
Total commands : 12
Input tokens   : 45,230
Output tokens  : 4,890
Saved          : 40,340  (89.2%)
```

## Step 4: Use the proxy for unsupported commands

Any command RTK doesn't know about runs through passthrough — the output is unchanged but usage is tracked:

```bash
rtk proxy make install
```

## What the hook does

Once installed, the hook intercepts every command your AI assistant runs and rewrites it transparently. You don't need to type `rtk` — the hook does it automatically.

For example, when Claude Code executes `cargo test`, the hook rewrites it to `rtk cargo test` before it runs. The filtered output is what the LLM sees.

## Next steps

- [Supported agents](./supported-agents.md) — Claude Code, Cursor, Copilot, and more
- [Commands](../commands/git.md) — full reference for each ecosystem
- [Configuration](../configuration.md) — customize RTK behavior
