# RTK - Rust Token Killer (Codex CLI)

**Usage**: Token-optimized CLI proxy for shell commands.

When installed with `rtk init --codex`, a trusted Codex `PreToolUse` hook
automatically rewrites eligible single, non-mutating Bash commands. These
instructions remain as a fallback for commands that retain Codex's native
approval flow and for shell paths that hooks do not intercept.

## Rule

Always prefix shell commands with `rtk`.

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
