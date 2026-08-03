# RTK - Rust Token Killer (Kimi Code CLI)

**Usage**: Token-optimized CLI proxy for shell commands (60-90% savings on dev operations).

## Rule

Always prefix shell commands with `rtk`.

Examples:

```bash
rtk git status
rtk cargo test
rtk npm run build
rtk pytest -q
rtk docker ps
```

## Meta Commands (always use rtk directly)

```bash
rtk gain              # Show token savings analytics
rtk gain --history    # Show recent command savings history
rtk proxy <cmd>       # Execute raw command without filtering (for debugging)
```

## Verification

```bash
rtk --version
rtk gain
which rtk
```

Run all shell commands through `rtk` to minimize token usage.
