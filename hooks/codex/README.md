# Codex CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Prompt-level guidance via awareness document -- no programmatic hook
- `rtk-awareness.md` is injected into `AGENTS.md` with an `@RTK.md` reference
- Installed to `$CODEX_HOME` when set, otherwise `~/.codex/`, by `rtk init --codex`
