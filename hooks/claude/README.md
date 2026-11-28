# Claude Code Hooks

> Part of [`hooks/`](../README.md)

## Specifics

- Shell-based `PreToolUse` hook -- requires `jq` for JSON parsing
- Returns `updatedInput` JSON for transparent command rewrite (agent doesn't know RTK is involved)
- Exits silently (exit 0) on any failure: jq missing, rtk missing, rtk too old (< 0.23.0), no match
- Version guard checks `rtk --version` against minimum 0.23.0
- `../rtk-awareness.md` (or `-high` / `-full`, per `awareness.level` in config.toml) is the instructions file embedded into CLAUDE.md by `rtk init`
