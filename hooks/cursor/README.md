# Cursor IDE Hooks

## Specifics

- Same delegating pattern as Claude Code hook but outputs Cursor's JSON format (`permission`/`updated_input` instead of `hookSpecificOutput`/`updatedInput`)
- Returns `{}` (empty JSON) when no rewrite applies -- Cursor requires JSON output for all code paths
- Requires `jq` and `rtk >= 0.23.0`
