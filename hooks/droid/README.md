# Factory Droid Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Shell-based `PreToolUse` hook — requires `jq` for JSON parsing
- Same delegating pattern and JSON format as Claude Code hook
- Factory Droid's `Execute` tool uses the same `.tool_input.command` field as Claude Code's `Bash` tool
- Output format is identical: `hookSpecificOutput` with `permissionDecision` and `updatedInput`
- Only difference: hook is registered for matcher `Execute` (not `Bash`) in `~/.factory/settings.json`
- Exits silently (exit 0) on any failure: jq missing, rtk missing, rtk too old (< 0.23.0), no match
- `rtk-awareness.md` is a slim instructions file that can be embedded into AGENTS.md by `rtk init --agent droid`

## Installation

```bash
rtk init -g --agent droid
```

This will:

1. Create `~/.factory/hooks/rtk-rewrite.sh` (executable)
2. Create `~/.factory/RTK.md` (slim awareness doc)
3. Patch `~/.factory/settings.json` to register the `PreToolUse` hook matching `Execute`

## Manual Setup

If automatic patching fails, add this to `~/.factory/settings.json`:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Execute",
        "hooks": [
          {
            "type": "command",
            "command": "~/.factory/hooks/rtk-rewrite.sh"
          }
        ]
      }
    ]
  }
}
```

## Testing

```bash
# Test the hook manually
echo '{"tool_name":"Execute","tool_input":{"command":"git status"}}' | ~/.factory/hooks/rtk-rewrite.sh

# Expected output:
# {"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","permissionDecisionReason":"RTK auto-rewrite","updatedInput":{"command":"rtk git status"}}}
```

## Uninstall

```bash
rtk init -g --agent droid --uninstall
```

Removes hook script, RTK.md, and settings.json entry.
