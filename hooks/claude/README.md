# Claude Code Hooks

> Part of [`hooks/`](../README.md)

## Specifics

- Shell-based `PreToolUse` hook -- requires `jq` for JSON parsing
- Returns `updatedInput` JSON for transparent command rewrite (agent doesn't know RTK is involved)
- Exits silently (exit 0) on any failure: jq missing, rtk missing, rtk too old (< 0.23.0), no match
- Version guard checks `rtk --version` against minimum 0.23.0
- `../rtk-awareness.md` (or `-high` / `-full`, per `awareness.level` in config.toml) is the instructions file embedded into CLAUDE.md by `rtk init`

## Bash sandbox

Claude Code's Bash sandbox cannot write RTK's data folder by default. Filtered
commands still run, but nothing is recorded for `rtk gain`, and output RTK cuts
cannot be stored for `rtk recall`; RTK then prints
`[rtk] recall store unavailable (...)` once per command instead of a recall hint.

To opt in, add the data folder to the sandbox's write list in
`~/.claude/settings.json` (macOS path shown; on Linux `~/.local/share/rtk`):

```json
"sandbox": { "filesystem": { "allowWrite": ["~/Library/Application Support/rtk"] } }
```

`RTK_DB_PATH` and `RTK_RECALL_DB` move the history and recall databases; if you
set them, allow their folders instead.
