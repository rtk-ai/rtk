# RTK - Rust Token Killer (Codex CLI)

**Usage**: Token-optimized CLI proxy for shell commands.

## Rule

Always prefix shell commands with `rtk`.

For file exploration and analysis, prefer `rtk read <file>` over native file-read tools.
Use `rtk read --symbol <name> <file>` for focused code context, `rtk read --changed <file>`
for repeat checks, and `rtk read -l none <file>` only when exact content is required for editing.

Examples:

```bash
rtk git status
rtk cargo test
rtk npm run build
rtk pytest -q
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
