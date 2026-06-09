# Codex CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Prompt-level guidance via awareness document -- no programmatic hook
- `rtk-awareness.md` is inlined into `AGENTS.md` so Codex receives the rules even when it does not expand `@...` references
- Installed to `$CODEX_HOME` when set, otherwise `~/.codex/`, by `rtk init --codex`
