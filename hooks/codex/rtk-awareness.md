# RTK - Rust Token Killer (Codex)

**Usage**: Token-optimized CLI proxy for shell commands in Codex.

## Rule

Always prefix RTK-covered shell commands with `rtk`.

Examples:

```bash
rtk git status
rtk cargo test
rtk npm run build
rtk pytest -q
rtk read src/main.rs
rtk grep "TODO" src
```

## Compound Commands

In compound commands, prefix each RTK-covered segment instead of only the first command.

```bash
rtk git add . && rtk cargo test
cd app && rtk npm test
```

## Windows / PowerShell

On Windows, prefer explicit RTK commands because Codex always reads `AGENTS.md`, while shell-hook support may vary by Codex environment.

- Use `rtk read`, `rtk grep`, and `rtk find` for file inspection
- Use `rtk git ...`, `rtk cargo ...`, `rtk pytest`, `rtk npm ...` for verbose workflows
- Keep PowerShell-native helpers for verification only, such as `Get-Command rtk`

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
```

```powershell
Get-Command rtk
```
