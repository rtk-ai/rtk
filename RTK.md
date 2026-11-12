# RTK - Rust Token Killer (Codex CLI)

**Usage**: Token-optimized CLI proxy for shell commands.

## Hook-Based Auto-Rewrite (Active)

Bash commands are automatically rewritten by the Codex PreToolUse hook.
Example: `git status` → `rtk git status` (transparent, no model awareness required).

You don't need to manually prefix commands — the hook handles it silently.

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
