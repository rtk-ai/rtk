---
inclusion: auto
---

# RTK - Rust Token Killer (Kiro)

**Usage**: Token-optimized CLI proxy for shell commands. Always use `rtk <cmd>` instead of raw commands to minimize token consumption (60-90% savings).

## Rule

Always prefix shell commands with `rtk` to minimize token consumption.

### Examples

```bash
rtk ls src/
rtk grep "pattern" src/
rtk find "*.rs" .
rtk git status
rtk cargo test
rtk docker ps
rtk gh pr list
```

## When NOT to use rtk

- Interactive commands that require user input (e.g., `vim`, `nano`)
- Commands that produce binary output (e.g., downloading files)
- Commands already prefixed with `rtk`
- Commands that rtk does not support

## Meta Commands

```bash
rtk gain              # Show token savings
rtk gain --history    # Command history with savings
rtk discover          # Find missed RTK opportunities
rtk proxy <cmd>       # Run raw (no filtering, for debugging)
```

## Why

RTK filters and compresses command output before it reaches the LLM context, saving 60-90% tokens on common operations. Always use `rtk <cmd>` instead of raw commands.
