<!-- rtk-instructions v3 -->
# RTK - Rust Token Killer (Windsurf)

**Usage**: Token-optimized CLI proxy for shell commands.

{{RTK_DIRECT_FIRST_POLICY}}

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

## Meta Commands

```bash
rtk gain              # Show token savings
rtk gain --history    # Command history with savings
rtk discover          # Find missed RTK opportunities
rtk proxy <cmd>       # Run raw (no filtering, for debugging)
```

## Why

RTK filters and compresses command output before it reaches the LLM context, cutting up to 90% of common command output. Always use `rtk <cmd>` instead of raw commands.
<!-- /rtk-instructions -->
