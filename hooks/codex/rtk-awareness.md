# RTK - Rust Token Killer (Codex CLI)

**Usage**: Token-optimized CLI proxy for shell commands.

## Rule

Run shell commands normally. The Codex `PreToolUse` hook transparently routes supported commands through `rtk`; commands without an RTK filter run unchanged.

You may invoke `rtk` explicitly for manual use, but do not add a second prefix to an already-prefixed command.

Examples:

```bash
rtk git status
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
which rtk
```
