# OpenCode Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- TypeScript plugin using the zx library (not a shell hook)
- Intercepts `tool.execute.before` events, calls `rtk rewrite` as a subprocess
- Uses `.quiet().nothrow()` to silently ignore failures
- Mutates `args.command` in-place if rewrite differs from original
- Installed to `~/.config/opencode/plugins/rtk.ts` by `rtk init -g --opencode`
