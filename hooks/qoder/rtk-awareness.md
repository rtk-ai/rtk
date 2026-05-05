# RTK — Qoder CLI Integration

**Usage**: Token-optimized CLI proxy (60-90% savings on dev operations)

## What's automatic

The `PreToolUse` hook in Qoder CLI's settings.json intercepts raw Bash tool calls
and rewrites them through RTK transparently via `updatedInput`.

The `@RTK.md` reference in `AGENTS.md` provides prompt-level guidance for the LLM
to proactively use `rtk` prefix on shell commands.

No shell scripts, no `jq` dependency — `rtk hook qoder` is a native Rust binary.

## Meta commands (always use directly)

```bash
rtk gain              # Token savings dashboard for this session
rtk gain --history    # Per-command history with savings %
rtk discover          # Scan session history for missed rtk opportunities
rtk proxy <cmd>       # Run raw (no filtering) but still track it
```

## Installation verification

```bash
rtk --version   # Should print: rtk X.Y.Z
rtk gain        # Should show a dashboard (not "command not found")
which rtk       # Verify correct binary path
```

> ⚠️ **Name collision**: If `rtk gain` fails, you may have `reachingforthejack/rtk`
> (Rust Type Kit) installed instead. Check `which rtk` and reinstall from rtk-ai/rtk.

## How the hook works

`rtk hook qoder` reads `PreToolUse` JSON from stdin and rewrites commands transparently:

1. Agent calls `Bash("git status")` → Qoder CLI fires `PreToolUse` hook
2. `rtk hook qoder` receives JSON, extracts `tool_input.command`
3. Returns `hookSpecificOutput.updatedInput.command = "rtk git status"`
4. Qoder CLI applies the updated input — agent sees RTK-filtered output directly

**Compound commands** (with `&&`, `||`, `|`, `;`) are handled by `rtk`'s internal
`rewrite_compound()` — each segment is rewritten independently, and pipe-incompatible
commands like `find`/`fd` are skipped automatically.
