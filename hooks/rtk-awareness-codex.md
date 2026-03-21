# RTK - Rust Token Killer (Codex CLI)

**Usage**: Token-optimized CLI proxy for shell-heavy commands in Codex.

Prefer explicit `rtk ...` commands for noisy shell output.

## Rule

Always prefix shell commands with `rtk`.

Use raw shell commands only when you need exact unfiltered output, exact line slices, or exact patch hunks.

Examples:

```bash
rtk git status
rtk git diff
rtk read src/main.rs
rtk grep "pattern" .
rtk cargo test
rtk npm run build
rtk pytest -q
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
rtk init --show --codex
which rtk
```
