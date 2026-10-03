# OpenCode Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- TypeScript plugin (not a shell hook)
- Supports both **OpenCode 2.0** (`execute.before` via `{ id, setup }`) and legacy **OpenCode 1.x** (`tool.execute.before`)
- Uses standard `node:child_process.execFile` (compatible with OpenCode CLI, OpenCode Desktop/Electron, and Windows; zero Bun/zx dependencies)
- Handles both exit code `0` and `3` from `rtk rewrite`
- Mutates command in-place (`event.input.command` in v2, `output.args.command` in v1)
- Robust binary discovery via `RTK_BIN`, system `PATH`, `~/.cargo/bin`, `~/.local/bin`, and Homebrew
- Installed to `~/.config/opencode/plugins/rtk.ts` by `rtk init -g --opencode`
