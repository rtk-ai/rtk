# Kilo Code Hooks

> Part of [`hooks/`](../README.md)

## Specifics

- Prompt-level guidance only (no programmatic hook) -- relies on Kilo Code reading custom instructions
- Installs `../rtk-awareness-full.md` (shared, agent-neutral): the instruction to prefix external programs with `rtk`, leaving shell builtins, keywords and the RTK-shadowed `env` unprefixed, plus meta commands. No hook, so the `full` level is always used regardless of `awareness.level`
- Installed to `.kilocode/rules/rtk-rules.md` (project-local) by `rtk init --agent kilocode`
