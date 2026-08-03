---
name: RTK Token-Optimized Commands
description: Prefer RTK command wrappers to reduce shell output token usage.
alwaysApply: true
---

# RTK - Rust Token Killer (Continue.dev)

RTK is a token-optimized CLI proxy for shell commands. Always prefix shell commands with `rtk`.

Examples:

```bash
rtk git status
rtk cargo test
rtk ls src/
rtk grep "pattern" src/
rtk find "*.rs" .
rtk docker ps
rtk gh pr list
```

Use these meta commands when needed:

```bash
rtk gain              # Show token savings
rtk gain --history    # Command history with savings
rtk discover          # Find missed RTK opportunities
rtk proxy <cmd>       # Run raw output for debugging
```

RTK passes unsupported commands through unchanged, so using the prefix is safe by default.
