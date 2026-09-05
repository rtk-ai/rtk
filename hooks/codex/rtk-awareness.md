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
```

## PowerShell cmdlets

PowerShell cmdlets (e.g., `Get-Content`, `Select-String`) are not standalone executables on PATH. Run them through `rtk powershell`:

```powershell
rtk powershell -NoProfile -Command "Get-Content -LiteralPath 'file.txt'"
```

Do NOT call cmdlets directly:

```
rtk Get-Content file.txt   # ✗ fails
```

## Meta Commands

```bash
rtk gain            # Token savings analytics
rtk gain --history  # Recent command savings history
rtk proxy <cmd>     # Run raw command without filtering
```

## Verification

```bash
rtk --version
rtk gain
which rtk
```
