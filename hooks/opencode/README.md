# OpenCode Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

### OpenCode v1 (default)

- TypeScript plugin using the `zx` library (bun-shell `$` helper)
- Intercepts `tool.execute.before` events, calls `rtk rewrite` as a subprocess
- Uses `.quiet().nothrow()` to silently ignore failures
- Mutates `args.command` in-place if rewrite differs from original
- Installed to `~/.config/opencode/plugins/rtk.ts` by `rtk init -g --opencode`

### OpenCode v2

- TypeScript plugin using the `Plugin.define` API (`@opencode-ai/plugin` v2)
- Registers `ctx.tool.hook("execute.before", ...)` instead of returning a hooks object
- Calls `rtk rewrite` via `child_process.spawn` (v2 context does not provide `$`)
- Uses `spawn` + stdout capture instead of exit codes (`rtk rewrite` exits non-zero on success)
- Mutates `event.input.command` in-place if rewrite differs from original
- Installed to `~/.config/opencode/plugins/rtk.ts` by `rtk init -g --opencode-v2`