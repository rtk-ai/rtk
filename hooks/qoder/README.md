# Qoder CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Uses the `rtk hook qoder` Rust binary (not a shell script) — no `jq` dependency
- Qoder CLI uses the same `PreToolUse` JSON protocol as Claude Code (`tool_name`/`tool_input` + `updatedInput`)
- Hook config written to `~/.qoder/settings.json` (global) or `.qoder/settings.json` (project)
- Transparent rewrite via `updatedInput` — no denial, no retry

## How it works

1. Agent runs `git status` → `rtk hook qoder` intercepts via `PreToolUse`
2. `rtk hook` reads JSON from stdin, matches tool_name `Bash`
3. Returns `hookSpecificOutput.updatedInput.command = "rtk git status"`
4. Qoder CLI rewrites the command transparently — agent sees the filtered output directly

## Testing

```bash
# Simulate Qoder CLI PreToolUse input
echo '{"tool_name":"Bash","tool_input":{"command":"git status"}}' | rtk hook qoder
# Should output JSON with updatedInput.command = "rtk git status"
```
