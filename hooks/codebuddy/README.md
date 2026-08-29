# CodeBuddy Code Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Uses the `rtk hook codebuddy` Rust binary — no `jq` dependency
- Same `PreToolUse` JSON protocol as Claude Code (`tool_name` + `tool_input.command`), with two differences: requires top-level `"continue": true` and a `"permissionDecision"` field (`"allow"`, `"ask"`, or `"deny"`)
- Emits `updatedInput` (not `modifiedInput` — CodeBuddy upstream has converged on `updatedInput`)
- `CODEBUDDY.md` is generated at install time from the shared `hooks/claude/rtk-awareness.md` template (no per-agent copy shipped in this directory)
- Installed globally via `rtk init -g --agent codebuddy`; there is no project-scoped variant

## Notes

- CodeBuddy is an independent product, not a Claude Code fork. The `PreToolUse` JSON shape is the only shared contract; if either agent evolves its schema, RTK tracks them independently
