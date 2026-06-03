# Codex CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Prompt-level guidance via awareness document remains supported
- `rtk-awareness.md` is injected into `AGENTS.md` with an `@RTK.md` reference
- `rtk hook codex` processes Codex `PreToolUse` JSON from stdin and emits a rewritten
  `updatedInput.command` or `updatedInput.cmd` when RTK supports the shell command
- Installed to `$CODEX_HOME` when set, otherwise `~/.codex/`, by `rtk init --codex`

## Native Hook

Add this to `$CODEX_HOME/hooks.json` (usually `~/.codex/hooks.json`) or the
equivalent inline `[hooks]` table in `config.toml`:

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
            "statusMessage": "Rewriting shell command with RTK"
          }
        ]
      }
    ]
  }
}
```

Codex requires non-managed hooks to be reviewed and trusted before they run.
Use `/hooks` in Codex to review the hook after adding it.

The parser accepts both `tool_input.command` and Codex's `tool_input.cmd`
shell-command field, preserving the original field name in `updatedInput`.
