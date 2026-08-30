# RTK - Rust Token Killer (Codex CLI)

**Usage**: Token-optimized CLI proxy for shell commands.

## Codex CLI Hook and Codex App Fallback

Codex CLI shell commands are automatically rewritten by the Codex PreToolUse hook.
Example: `git status` → `rtk git status` (transparent, no model awareness required).

Codex App internal or programmatic tool calls may bypass CLI `hooks.json`. In Codex App,
prefix eligible external CLI commands with `rtk` inside the shell invocation:

```bash
rtk git status
rtk cargo test
rtk npm run build
```

Keep unsupported commands and PowerShell cmdlets native so their semantics are unchanged.

## Meta Commands (manually prefixed)

```bash
rtk gain              # Show token savings analytics
rtk gain --history    # Show command usage history with savings
rtk discover          # Analyze Codex history for missed opportunities
rtk proxy <cmd>       # Execute raw command without filtering (for debugging)
```

## Windows PowerShell Commands

For PowerShell cmdlets and scripts, wrap the PowerShell process explicitly:

```powershell
rtk powershell -NoProfile -Command "Get-Content -LiteralPath 'C:\path\file.txt'"
rtk powershell -NoProfile -File path\to\script.ps1
```

## Verification

```bash
rtk --version         # Should show: rtk X.Y.Z
rtk gain              # Should work (not "command not found")
```

⚠️ **Name collision**: If `rtk gain` fails, you may have reachingforthejack/rtk (Rust Type Kit) installed instead.
