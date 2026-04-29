# Kiro CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Two-layer integration: steering file (prompt-level) + `preToolUse` hook (deny-with-suggestion enforcement)
- `rules.md` contains the instruction to prefix all shell commands with `rtk`, usage examples, and meta commands
- Installed to `~/.kiro/steering/rtk-rules.md` (global) by `rtk init -g --agent kiro-cli`
- Installed to `.kiro/steering/rtk-rules.md` (project-local) by `rtk init --agent kiro-cli`
- Agent config with `preToolUse` hook installed to `~/.kiro/agents/rtk-hook.json` (global) or `.kiro/agents/rtk-hook.json` (project)
- Hook matches `execute_bash` and `shell` tools, runs `rtk hook kiro` to deny-with-suggestion for rewritable commands
- Kiro CLI automatically loads all `.md` files from `~/.kiro/steering/` into every conversation context
