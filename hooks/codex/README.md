# Codex CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Prompt-level guidance via awareness document is the guaranteed Codex integration path today
- RTK also exposes an experimental `rtk hook codex` entry for Codex environments that support lifecycle hook execution
- `rtk-awareness.md` is injected into `AGENTS.md` with an `@RTK.md` reference
- Installed to `$CODEX_HOME` when set, otherwise `~/.codex/`, by `rtk init --codex`
- On Windows, the installed `RTK.md` includes PowerShell-friendly verification guidance and explicit `rtk ...` usage rules
