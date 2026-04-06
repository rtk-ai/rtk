---
title: RTK Documentation
description: RTK (Rust Token Killer) — reduce LLM token consumption by 60-90% on common dev commands, with zero workflow changes
sidebar:
  order: 1
---

# RTK — Rust Token Killer

RTK is a CLI proxy that sits between your AI assistant and your development tools. It filters command output before it reaches the LLM, keeping only what matters and discarding boilerplate, progress bars, and noise.

**Result:** 60-90% fewer tokens consumed per command, without changing how you work. You run `git status` as usual — RTK's hook intercepts it, filters the output, and the LLM sees a compact 3-line summary instead of 40 lines.

## How it works

```
Your AI assistant runs:  git status
                              ↓
              Hook intercepts (PreToolUse)
                              ↓
              rtk git status  (transparent rewrite)
                              ↓
     Raw output: 40 lines     →     Filtered: 3 lines
     ~800 tokens              →     ~60 tokens  (92% saved)
                              ↓
              LLM sees the compact output
```

Zero config changes to your workflow. The hook handles everything automatically.

## What RTK optimizes

60+ commands across 9 ecosystems — Git, Cargo/Rust, JavaScript, Python, Go, Ruby, .NET, Docker/Kubernetes, and more. See [What RTK Optimizes](./what-rtk-covers.md) for the full list with savings percentages.

## Get started

1. **[Installation](./getting-started/installation.md)** — Install RTK and verify you have the right package
2. **[Quick Start](./getting-started/quick-start.md)** — Connect to your AI assistant in 5 minutes
3. **[Supported Agents](./getting-started/supported-agents.md)** — Claude Code, Cursor, Copilot, Gemini, and 7 more

## Measure your savings

```bash
rtk gain           # total savings across all sessions
rtk gain --daily   # day-by-day breakdown
rtk gain --weekly  # weekly aggregation
```

See [Analytics](./analytics/gain.md) for export formats and analysis workflows.

## Further reading

- [Configuration](./getting-started/configuration.md) — config.toml, global flags, env vars, tee recovery
- [Troubleshooting](./troubleshooting.md) — common issues and fixes
- [ARCHITECTURE.md](https://github.com/rtk-ai/rtk/blob/master/ARCHITECTURE.md) — system design for contributors
