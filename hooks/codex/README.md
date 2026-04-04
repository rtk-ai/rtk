# Codex CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Prompt-level guidance via `rtk-awareness.md`, injected into `AGENTS.md` with an `@RTK.md` reference
- Programmatic Codex hook via `rtk hook codex`, wired through `hooks.json`
- `rtk init --codex` also enables `features.codex_hooks = true` in the active Codex `config.toml`

## Behavior

- Codex `PreToolUse` hooks currently **cannot** apply `updatedInput` yet.
- RTK therefore uses a **deny-with-suggestion** pattern for Codex:
  - raw command: `git status`
  - hook response: deny + suggest `rtk git status`
  - Codex retries with the RTK command and gets filtered output

This differs from Claude Code and Cursor, where RTK can transparently rewrite the Bash command in-place.
