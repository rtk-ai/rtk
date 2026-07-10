# RTK - Rust Token Killer (Codex CLI)

**Usage**: Token-optimized CLI proxy for shell commands.

## Rule

Always prefix shell commands with `rtk`.

Examples:

```bash
rtk git status
rtk cargo test
rtk npm run build
rtk pytest -q
rtk python -m http.server
rtk dotnet test
```

## Meta Commands

```bash
rtk gain            # Token savings analytics
rtk gain --history  # Recent command savings history
rtk proxy <cmd>     # Run raw command without filtering
```

## Windows Notes (Codex Desktop App)

On Windows, any command prefixed with `rtk` works, including
PowerShell cmdlets, aliases, and shell built-ins:

```powershell
rtk Get-ChildItem   # PowerShell cmdlet — OK
rtk dir             # PowerShell alias — OK
rtk echo hello      # shell built-in — OK
rtk type file.txt   # PowerShell built-in — OK  
rtk where node      # PowerShell alias for Get-Command — OK
```

To run a command without RTK token optimisation, use:
```bash
rtk proxy <command>
```

## Verification

```bash
rtk --version
rtk gain
where rtk           # Windows (powershell)
```
