# Devin Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Devin hooks are fully compatible with Claude Code hooks and use the same JSON format
- Shell-based `PreToolUse` hook -- requires `jq` for JSON parsing
- Returns `updatedInput` JSON for transparent command rewrite (agent doesn't know RTK is involved)
- Exits silently (exit 0) on any failure: jq missing, rtk missing, rtk too old (< 0.23.0), no match
- Version guard checks `rtk --version` against minimum 0.23.0
- Uses the standard `rtk hook claude` command (Devin and Claude Code share the same hook protocol)

## Installation

```bash
rtk init --agent devin
```

This installs the hook in `~/.config/devin/config.json` using the Claude-compatible format.

## Testing

```bash
# Run the full test suite (60+ assertions)
bash hooks/devin/test-rtk-rewrite.sh

# Test against a specific hook path
HOOK=/path/to/rtk-rewrite.sh bash hooks/devin/test-rtk-rewrite.sh

# Enable audit logging during testing
RTK_HOOK_AUDIT=1 RTK_AUDIT_DIR=/tmp bash hooks/devin/test-rtk-rewrite.sh
```
