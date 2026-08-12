# Codex CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Native Rust `PreToolUse` processor: `rtk hook codex`
- Transparently rewrites `tool_input.command` with Codex's `updatedInput` response
- Registers a `Bash` matcher in `.codex/hooks.json` (project) or `$CODEX_HOME/hooks.json` (global)
- Keeps `rtk-awareness.md` in `AGENTS.md` through an `@RTK.md` reference for RTK meta-command guidance
- Installed by `rtk init --codex` (project) or `rtk init -g --codex` (global)

Codex requires `permissionDecision: "allow"` in the hook response for `updatedInput` to take effect. Codex applies the replacement before its normal command approval and sandbox checks, so those native checks still run on the rewritten command.

No match, malformed JSON, unsupported commands, heredocs, substitutions, and file redirections fail open: the hook exits successfully without stdout and Codex executes the original command.
