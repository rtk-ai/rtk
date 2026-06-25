# Devin Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Devin for Terminal uses the same `PreToolUse` JSON protocol as Claude Code
- Binary hook: `rtk hook devin` (implemented in `src/hooks/hook_cmd.rs`, delegates to the Claude payload processor)
- No shell script, `jq`, or version guard required — the Rust binary handles all JSON I/O
- Returns `updatedInput` JSON for transparent command rewrite (agent doesn't know RTK is involved)
- Devin invocations are distinguishable from Claude Code in logs and analytics thanks to the dedicated `rtk hook devin` entry point

## Installation

```bash
rtk init --agent devin
```

This patches `~/.config/devin/config.json` with a `PreToolUse` hook entry pointing at `rtk hook devin`.

## Uninstall

```bash
rtk init --agent devin --uninstall
```

Removes the RTK hook entry from `~/.config/devin/config.json` (a `.json.bak` backup is created).
