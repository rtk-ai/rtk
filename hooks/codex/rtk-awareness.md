# RTK - Rust Token Killer (Codex CLI)

**Usage**: Token-optimized CLI proxy for shell commands.

## Rule

Prefix supported executable commands with `rtk`. Do not prefix shell builtins such as `cd`
or `export`; in a compound command, prefix only the executable command:

Examples:

```bash
rtk git status
rtk cargo test
rtk npm run build
rtk pytest -q
cd backend && rtk cargo test
```

Use `rtk rewrite` when the correct placement is unclear. For example,
`rtk rewrite 'cd backend && uv run pytest tests/'` prints
`cd backend && uv run rtk pytest tests/`.

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

An error prefixed with `[rtk:` came from RTK and does not mean the binary is unavailable.
Run the checks above before falling back to raw commands.
