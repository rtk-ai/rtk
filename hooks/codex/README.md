# Codex CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Command-based `PreToolUse` hook via Codex `hooks.json`.
- Returns `hookSpecificOutput.updatedInput.command` to rewrite shell commands before execution.
- Codex accepts `updatedInput` only with `permissionDecision: "allow"`, so RTK uses a dedicated `rtk hook codex` processor.
- `rtk-awareness.md` is still injected into `AGENTS.md` with an `@RTK.md` reference as prompt-level backup guidance.
- Installed locally to `.codex/hooks.json` by `rtk init --codex`, or globally to `$CODEX_HOME/hooks.json` / `~/.codex/hooks.json` by `rtk init -g --codex`.

## Hook entry

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "^Bash$",
        "hooks": [
          {
            "type": "command",
            "command": "rtk hook codex",
            "timeout": 10,
            "statusMessage": "RTK rewriting shell command"
          }
        ]
      }
    ]
  }
}
```
