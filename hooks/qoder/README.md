# Qoder IDE Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Shell-based `PreToolUse` hook (powered natively by `rtk hook qoder`)
- Returns `updatedInput` JSON for transparent command rewrite (IDE doesn't know RTK is involved)
- Protocol is identical to Claude Code.
- Exits silently (exit 0) on any failure or JSON parse error.
- `rtk-awareness.md` is embedded into `~/.qoder/RTK.md` by `rtk init -g --agent qoder`.