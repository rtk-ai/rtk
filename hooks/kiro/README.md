# Kiro CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Prompt-level guidance only (no programmatic hook) — relies on Kiro CLI reading steering files
- `rules.md` contains the instruction to prefix all shell commands with `rtk`, usage examples, and meta commands
- Installed to `~/.kiro/steering/rtk-rules.md` (global) by `rtk init -g --agent kiro`
- Installed to `.kiro/steering/rtk-rules.md` (project-local) by `rtk init --agent kiro`
- Kiro CLI automatically loads all `.md` files from `~/.kiro/steering/` into every conversation context
