# RTK - Rust Token Killer

**Usage**: Token-optimized CLI proxy (60-90% savings on dev operations)

## Meta Commands (always use rtk directly)

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

## Hook-Based Usage

All other commands are automatically rewritten by the Claude Code hook.
Example: `git status` → `rtk git status` (transparent, 0 tokens overhead)

## Reading the full output

RTK output is a filtered summary — for most commands it keeps what matters
(test failures, panics, and build errors are preserved). When you need the
complete, unfiltered output to confirm a claim (tests really passed, build is
really clean) rather than skim it, get the raw output instead of trusting the
summary:

- Run `rtk proxy <cmd>` to re-run the command with no filtering.
- Or, when the filtered output ends with `[full output: <path>]`, read that
  file directly — it is the complete, unfiltered log already on disk.

Run `rtk --help` for the full command reference.
