# Codex CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Native `PreToolUse` hooks rewrite supported shell commands through `rtk hook codex`
- Codex only accepts rewritten input together with `permissionDecision: "allow"`, so RTK rewrites explicit allow-rule matches and defers ask/default commands to Codex's native permission flow
- The global hook preserves existing Codex hooks and adds the current `Bash` matcher plus `Shell` and `PowerShell` compatibility matchers
- `rtk init -g --codex` installs the hook into `$CODEX_HOME/hooks.json` when set, otherwise `~/.codex/hooks.json`
- `rtk init --codex` is project-scoped guidance only: it injects `RTK.md` into local `AGENTS.md` with an `@RTK.md` reference, but project-local Codex configs do not install hooks
