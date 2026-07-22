# RTK - Rust Token Killer (Grok Build)

Token-optimized CLI proxy for shell commands.

## Rule (mandatory)

When using shell tools, **always** prefix supported commands with `rtk`:

| Instead of | Use |
|------------|-----|
| `git status` | `rtk git status` |
| `cargo test` | `rtk cargo test` |
| `npm run build` | `rtk npm run build` |
| `pytest -q` | `rtk pytest -q` |

Do this by default. Prefer `rtk <cmd>` over bare `<cmd>`.

**Exceptions** (only then skip `rtk`):
- The user explicitly asks for raw/unfiltered output
- `rtk` is unavailable on PATH

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
