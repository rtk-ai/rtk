<!-- rtk-instructions -->
# RTK - Rust Token Killer

**Usage**: Token-optimized CLI proxy (cuts up to 90% of bash output)

## How Devin CLI uses RTK

The Devin CLI `PreToolUse` hook automatically rewrites supported shell commands to `rtk <command>` before execution. Unsupported commands pass through unchanged, so prefixing with `rtk` is always safe.

## Meta Commands (always use rtk directly)

```bash
rtk gain              # Show token savings analytics
rtk gain --history    # Show command usage history with savings
rtk discover          # Analyze Devin CLI history for missed opportunities
rtk proxy <cmd>       # Execute raw command without filtering (for debugging)
```

## Installation Verification

```bash
rtk --version         # Should show: rtk X.Y.Z
rtk gain              # Should work (not "command not found")
which rtk             # Verify correct binary
```

⚠️ **Name collision**: If `rtk gain` fails, you may have reachingforthejack/rtk (Rust Type Kit) installed instead.

## Common Commands to Run Through RTK

```bash
rtk git status
rtk git log
rtk git diff
rtk git add
rtk git commit
rtk git push
rtk cargo test
rtk cargo build
rtk cargo clippy
rtk jest
rtk vitest
rtk pytest
rtk ls
rtk grep
rtk find
rtk read
rtk docker ps
rtk docker logs
rtk kubectl get
rtk kubectl logs
rtk gh pr view
rtk gh run list
```

Prefix any shell command with `rtk` to get compact output. If RTK has no filter for it, the command runs unchanged.

## Permission Sync

RTK reads `permissions.allow/ask/deny` from Devin CLI's own config files (`.devin/config.json`, `.devin/config.local.json`, `~/.config/devin/config.json`). Allowed commands are auto-approved and rewritten; denied commands are blocked; all others are rewritten so Devin CLI can prompt on the rewritten command.
<!-- /rtk-instructions -->