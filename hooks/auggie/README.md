# Augment Code (Auggie) Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Rust binary hook (`rtk hook auggie`) — native handler, no external dependencies
- PreToolUse hook with `launch-process` matcher (NOT `Bash`)
- Returns `updatedInput` JSON for transparent command rewrite (agent doesn't know RTK is involved)
- Payload format matches Claude Code: `tool_input.command` in, `hookSpecificOutput.updatedInput.command` out
- Exits silently (exit 0) on any failure: jq missing, rtk missing, no match

## Installation

```bash
rtk init --agent auggie    # patches ~/.augment/settings.json
rtk init --agent auggie --show    # check status
rtk init --agent auggie --uninstall    # remove
```

## JSON Format

**Input** (stdin):
```json
{
  "tool_name": "launch-process",
  "tool_input": { "command": "git status" }
}
```

**Output** (stdout, when rewritten):
```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "allow",
    "permissionDecisionReason": "RTK auto-rewrite",
    "updatedInput": { "command": "rtk git status" }
  }
}
```
