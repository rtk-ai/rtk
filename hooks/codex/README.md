# Codex CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Native `PreToolUse` hook delegates Bash commands to `rtk hook codex`
- Rewrites are returned through Codex's `updatedInput` protocol; commands with no RTK rewrite emit no output
- Existing `hooks.json` entries are preserved and repeated installation is idempotent
- `rtk-awareness.md` is installed as `RTK.md` and referenced from `AGENTS.md` for usage guidance
- Global installation uses `$CODEX_HOME` when set, otherwise `~/.codex/`; local installation uses `.codex/hooks.json`
- After installation, review and trust the hook with `/hooks`, then restart Codex
