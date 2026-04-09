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

## Step 2: Use your tools normally

Once the hook is installed, nothing changes in how you work. Your AI assistant runs commands as usual — the hook intercepts them transparently and rewrites them before execution.

For example, when Claude Code runs `cargo test`, the hook rewrites it to `rtk cargo test` before it executes. The LLM receives filtered output with only the failures — not 500 lines of passing tests. You never see or type `rtk`.

Supported ecosystems: Git, Cargo/Rust, JavaScript (vitest, tsc, eslint, pnpm, Next.js, Prisma), Python, Go, Ruby, .NET, Docker/Kubernetes, GitHub CLI, and more. See [What RTK Optimizes](../what-rtk-covers.md) for the full list.

## Step 3: Check your savings

After a few commands, see how much was saved:

```bash
rtk gain
```

```
Total commands : 12
Input tokens   : 45,230
Output tokens  : 4,890
Saved          : 40,340  (89.2%)
```

## Step 4: Unsupported commands

Commands RTK doesn't recognize run through passthrough — output is unchanged, usage is tracked:

```bash
rtk proxy make install
```

## Next steps

- [What RTK Optimizes](../what-rtk-covers.md) — all supported commands and savings by ecosystem
- [Supported agents](./supported-agents.md) — Claude Code, Cursor, Copilot, and more
- [Configuration](./configuration.md) — customize RTK behavior
