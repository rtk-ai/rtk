<!-- rtk-instructions v1 -->
# RTK - Rust Token Killer (ZCode Agent)

Always prefix shell commands with `rtk` to minimize token consumption.

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
rtk gain --history    # Show command usage history
rtk discover          # Find missed RTK opportunities
rtk proxy <cmd>       # Run raw output for debugging
```

RTK filters and compresses command output before it reaches the agent context,
saving 60-90% tokens on common development operations. Use `rtk <cmd>` instead
of raw commands whenever possible.
<!-- /rtk-instructions -->
