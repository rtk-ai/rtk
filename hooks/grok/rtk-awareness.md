# RTK - Rust Token Killer (Grok CLI)

**Usage**: Token-optimized CLI proxy for shell commands.

## Rule

Always prefix shell commands with `rtk`.

When a PreToolUse hook denies a command with an RTK suggestion, re-run the **exact** suggested command (do not retry the original raw command).

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
rtk rewrite <cmd>   # Preview how a command would be rewritten
```

## Verification

```bash
rtk --version
rtk gain
which rtk
```
