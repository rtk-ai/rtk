# Codex CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Prompt-level guidance via awareness document -- no programmatic hook
- `../rtk-awareness-full.md` is written to `RTK.md` and referenced from `AGENTS.md` via `@RTK.md`. No hook, so the `full` level is always used regardless of `awareness.level`
- Installed to `$CODEX_HOME` when set, otherwise `~/.codex/`, by `rtk init --codex`
