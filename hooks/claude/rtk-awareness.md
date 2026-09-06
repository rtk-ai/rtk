# RTK - Rust Token Killer

**Usage**: Token-optimized CLI proxy (cuts up to 90% of bash output)

## Meta Commands (type these with `rtk` directly)

```bash
rtk gain              # Show token savings analytics
rtk gain --history    # Show command usage history with savings
rtk discover          # Analyze Claude Code history for missed opportunities
rtk proxy <cmd>       # Execute raw command without filtering (for debugging)
```

## Installation Verification

```bash
rtk --version         # Should show: rtk X.Y.Z
rtk gain              # Should work (not "command not found")
which rtk             # Verify correct binary
```

⚠️ **Name collision**: If `rtk gain` fails, you may have reachingforthejack/rtk (Rust Type Kit) installed instead.

## Hook-Rewritten Commands

For git, gh, cargo, and other hook-covered tools, type the plain command without an `rtk` prefix. The Claude Code hook rewrites it after permission checks.

Example: type `git status`. The hook transparently runs `rtk git status`; do not type the rewritten form yourself.

Refer to CLAUDE.md for full command reference.
