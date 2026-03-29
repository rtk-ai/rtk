# Factory AI Droid Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Shell-based `PreToolUse` hook with `Execute` matcher — requires `jq` for JSON parsing
- Returns `updatedInput` JSON for transparent command rewrite (Droid doesn't know RTK is involved)
- Exits silently (exit 0) on any failure: jq missing, rtk missing, rtk too old (< 0.23.0), no match
- Version guard checks `rtk --version` against minimum 0.23.0
- Hook format is identical to Claude Code (same `hookSpecificOutput` JSON structure)
- Config: `~/.factory/settings.json` with `PreToolUse` → `Execute` matcher
