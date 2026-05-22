# Codex CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Native `PreToolUse` hook via `hooks.json`
- Returns `hookSpecificOutput.updatedInput` for transparent command rewrite
- `rtk-awareness.md` is injected into `AGENTS.md` with an `@RTK.md` reference
- Installed to project `.codex/hooks.json` by `rtk init --codex`
- Installed to `$CODEX_HOME/hooks.json` when set, otherwise `~/.codex/hooks.json`, by `rtk init -g --codex`
