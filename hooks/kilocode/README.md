# Kilo Code Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Prompt-level guidance only (no programmatic hook) -- relies on Kilo Code reading custom instructions
- `rules.md` contains the instruction to prefix all shell commands with `rtk`, usage examples, and meta commands
- Installed to `.kilocode/rules/rtk-rules.md` (project-local) by `rtk init --agent kilocode`
