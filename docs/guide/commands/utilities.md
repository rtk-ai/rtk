---
title: Utilities
description: RTK utility commands — proxy passthrough, analytics, global flags
sidebar:
  order: 12
---

# Utilities

Utility commands that apply across all ecosystems.

## proxy — passthrough with tracking

```bash
rtk proxy <command...>
```

Runs any command without filtering but records it in the token savings database. Useful when you need raw output but still want usage tracked.

```bash
rtk proxy git log --oneline -20    # full git log, no filtering
rtk proxy npm install express      # raw npm output
rtk proxy curl https://api.example.com/data
```

All proxy commands appear in `rtk gain --history` with 0% savings (input = output).

## Global flags

These flags apply to every RTK command:

| Flag | Description |
|------|-------------|
| `-v` | Debug messages |
| `-vv` | Show command being executed |
| `-vvv` | Show raw output before filtering |
| `-u` / `--ultra-compact` | Maximum compression (ASCII icons, inline format) |

**Verbosity example:**
```bash
rtk git status -vvv
# shows the raw git status output before RTK filters it
```

**Ultra-compact example:**
```bash
rtk git push -u
# output: ✓ main  (vs "ok main" in normal mode)
```

## RTK_DISABLED — per-command override

```bash
RTK_DISABLED=1 git status    # runs raw git status, no rewrite
```

## Passthrough behavior

Any command RTK doesn't recognize is executed unchanged and the output passes through:

```bash
rtk make install     # runs make install verbatim
rtk terraform plan   # runs terraform plan verbatim (unless a TOML filter matches)
```

The exit code from the underlying command is always preserved.
