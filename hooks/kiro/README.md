# Kiro CLI Hooks

> Part of [`hooks/`](../README.md)

## Specifics

- Prompt-level guidance only (no programmatic hook) -- relies on Kiro CLI loading steering files
- Kiro's `PreToolUse`/`PreTaskExec` hooks are gate-only: they can block a command (exit code 2) but
  cannot rewrite it, so `git status` -> `rtk git status` is impossible via a hook. RTK installs the
  shared `../rtk-awareness-full.md` (agent-neutral) into Kiro's steering directory instead, which the
  agent auto-loads as context. No hook, so the `full` level is always used regardless of `awareness.level`
- Installed to `.kiro/steering/rtk.md` (project-local) by `rtk init --agent kiro`, or to
  `~/.kiro/steering/rtk.md` (global) by `rtk init -g --agent kiro`
