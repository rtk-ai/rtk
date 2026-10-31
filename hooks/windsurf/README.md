# Devin Desktop (formerly Windsurf) Rules

> Part of [`hooks/`](../README.md)

## Specifics

- Prompt-level guidance only (no programmatic hook) -- relies on Devin Desktop reading rules files
- Installs `../rtk-awareness-full.md` (shared, agent-neutral): the instruction to prefix every shell command with `rtk`, plus meta commands. No hook, so the `full` level is always used regardless of `awareness.level`
- Installed to `.windsurfrules` (project-local, workspace-scoped) by `rtk init -g --agent windsurf`
- RTK retains the `windsurf` agent identifier. [Devin Desktop still supports `.windsurfrules`](https://docs.devin.ai/desktop/devin-desktop-faq#are-my-windsurf-rules-still-supported-what-about-devin-rules)
