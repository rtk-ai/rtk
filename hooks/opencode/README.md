# OpenCode Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- TypeScript plugin using Node's cross-platform `child_process.execFile` API (not a shell hook)
- Intercepts `tool.execute.before` events, calls `rtk rewrite` as a subprocess
- Passes subprocess arguments without a shell and treats empty stdout as no rewrite
- Mutates `args.command` in-place if rewrite differs from original
- Installed to `~/.config/opencode/plugins/rtk.ts` by `rtk init -g --opencode`
