---
title: RTK Guide
description: User-facing documentation for RTK, the token-saving CLI proxy for AI coding assistants
sidebar:
  order: 1
---

# RTK Guide

RTK (Rust Token Killer) is a CLI proxy that reduces LLM token consumption by 60-90% on common development operations. It filters and compresses command output before it reaches your AI assistant, without changing how you work.

## What's in this guide

- **[Getting Started](./getting-started/installation.md)** — Install RTK, verify it works, run your first command
- **Commands** — Per-ecosystem reference: git, cargo, GitHub CLI, JavaScript, Python, and more
- **[Filters](./filters/using-filters.md)** — Create custom TOML filters for your own commands
- **[Analytics](./analytics/gain.md)** — Measure your actual token savings with `rtk gain`
- **[Configuration](./configuration.md)** — Customize RTK behavior via `~/.config/rtk/config.toml`
- **[Troubleshooting](./troubleshooting.md)** — Common issues and how to fix them
