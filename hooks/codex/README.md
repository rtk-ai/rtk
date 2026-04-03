# Codex CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Prompt-level guidance via awareness document -- no programmatic hook
- Global installs inject the absolute `~/.codex/RTK.md` path into `AGENTS.md`; local installs keep the project-local `@RTK.md` reference
- Installed to `~/.codex/` by `rtk init --codex`
