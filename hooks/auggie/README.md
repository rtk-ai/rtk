# Auggie (Augment Code CLI) Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Native Rust binary hook (`rtk hook auggie`) — no shell script, no `jq` dependency
- Reuses Claude Code's `PreToolUse` JSON shape (`{tool_name, tool_input.command}` in, `hookSpecificOutput.updatedInput.command` out) since Auggie mirrors that schema
- Registered in `~/.augment/settings.json` under `hooks.PreToolUse` with `matcher: "launch-process"` (Auggie's shell tool, not `Bash`)
- Exits 0 on every path so failures never block the agent's command
- See Auggie's hook reference: <https://docs.augmentcode.com/cli/hooks>

## Install

```bash
rtk init -g --agent auggie         # registers ~/.augment/settings.json hook
rtk init --show                    # status (now includes an Auggie line)
rtk init -g --agent auggie --uninstall   # remove just the Auggie entry
rtk init -g --uninstall            # remove all RTK artifacts (Claude + Cursor + Auggie + …)
```

## What ends up in `~/.augment/settings.json`

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "launch-process",
        "hooks": [
          { "type": "command", "command": "rtk hook auggie" }
        ]
      }
    ]
  }
}
```

The installer is idempotent: re-running `rtk init -g --agent auggie` won't add a duplicate entry, and existing unrelated hooks in the file are preserved (the file is JSON-merged, not overwritten). A `.bak` is written next to `settings.json` whenever the file is modified.

## `updatedInput` support

Auggie's hook docs note that, as of writing, modifying tool input via `updatedInput` is not yet implemented — only blocking with `permissionDecision: "deny"` is wired up. RTK's hook still emits the Claude-Code-shaped `hookSpecificOutput.updatedInput` payload so that auto-rewrite turns on transparently as soon as Auggie's `updatedInput` support lands. Until then the hook is a safe no-op (it never denies, never blocks).

## Manual test

```bash
echo '{"hook_event_name":"PreToolUse","tool_name":"launch-process","tool_input":{"command":"git status"}}' \
  | rtk hook auggie
# -> {"hookSpecificOutput":{"hookEventName":"PreToolUse",...,"updatedInput":{"command":"rtk git status"}}}
```

No output (and exit 0) means the command was not rewriteable, which is the expected pass-through behaviour.
