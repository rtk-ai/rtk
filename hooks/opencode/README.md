# OpenCode Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- TypeScript plugin using the zx library (not a shell hook)
- Intercepts `tool.execute.before` events, calls `rtk hook opencode` as a subprocess
- The Rust side judges the command against OpenCode's own permission rules
  (root and project `opencode.json`/`.jsonc`, last match wins) and answers `{}`
  whenever the rewrite would change the verdict those rules give — OpenCode
  evaluates the final command itself, so RTK never lifts a deny, silences an
  ask, or blocks an allow (#4195)
- Uses `.quiet().nothrow()` to silently ignore failures
- Mutates `args.command` in-place if the answered rewrite differs from original
- Installed to `~/.config/opencode/plugins/rtk.ts` by `rtk init -g --opencode`
