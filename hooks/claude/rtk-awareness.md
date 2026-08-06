# RTK - Rust Token Killer

**Usage**: Token-optimized CLI proxy (cuts up to 90% of bash output)

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

## When You Need the Full Output

Filtering is not lossy — it is deferred. The complete unmodified output is
written to disk and its path is printed at the end of the compact output:

```
[full output: <platform tee dir>/<id>_<cmd>.log]
```

This happens on failures by default, or on every command with
`[tee] mode = "always"` in `config.toml`.

**Read that path first.** It is byte-for-byte ground truth, and the `Read`
tool bypasses the hook entirely, so retrieving it costs nothing extra.

If you need raw output *in the same call* — chasing a bug through warnings, or
parsing output whose exact shape matters — prefix the command:

```bash
RTK_DISABLED=1 <command>    # skip rewriting for this one call
```

Use it deliberately. The tee file already covers most "I need the full output"
cases, and disabling the filter gives up the token savings that make long
sessions possible.

Refer to CLAUDE.md for full command reference.
