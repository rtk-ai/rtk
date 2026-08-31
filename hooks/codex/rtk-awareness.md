# RTK - Rust Token Killer (Codex CLI)

**Usage**: Token-optimized CLI proxy for shell commands.

{{RTK_DIRECT_FIRST_POLICY}}

```bash
# Wrong
rtk proxy pwsh -Command "git status"

# Correct
rtk git status
rtk cargo test
rtk read src/main.rs
rtk rg "TODO" src
```

## Meta Commands

```bash
rtk gain            # Token savings analytics
rtk gain --history  # Recent command savings history
rtk proxy <cmd>     # Run raw command without filtering
```

## Verification

```bash
rtk --version
rtk gain
which rtk
```
