# Cline / Roo Code Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Prompt-level guidance only (no programmatic hook) -- relies on Cline reading custom instructions
- Installs `../rtk-awareness-full.md` (shared, agent-neutral): the instruction to prefix every shell command with `rtk`, plus meta commands. No hook, so the `full` level is always used regardless of `awareness.level`
- Installed to `.clinerules` (project-local) by `rtk init`
