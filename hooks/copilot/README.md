# GitHub Copilot Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Uses the `rtk hook copilot` Rust binary (not a shell script) -- no `jq` dependency
- Auto-detects two input formats: VS Code Copilot Chat (snake_case `tool_name`/`tool_input`) and Copilot CLI (camelCase `toolName`/`toolArgs` with JSON-stringified args)
- VS Code format: returns `updatedInput` for transparent rewrite
- Copilot CLI format: returns `permissionDecision: "deny"` with suggestion (Copilot CLI API doesn't support `updatedInput`)

## Testing

```bash
bash hooks/test-copilot-rtk-rewrite.sh
```
