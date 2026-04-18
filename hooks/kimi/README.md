# Kimi Code CLI Hook

Native Rust binary hook for Kimi Code CLI. Reuses the Claude hook processor (`process_claude_payload`) since Kimi's `PreToolUse` JSON format and structured output are compatible.

## Installation

```bash
rtk init -g --kimi
```

This patches `~/.kimi/config.toml` to add a `PreToolUse` hook:

```toml
[[hooks]]
event = "PreToolUse"
matcher = "Shell"
command = "rtk hook kimi"
timeout = 5
```

## How It Works

1. Kimi fires `PreToolUse` before executing any `Shell` tool call
2. Kimi passes JSON via stdin to `rtk hook kimi`
3. RTK extracts the command, looks up the rewrite registry
4. If a match is found, RTK outputs structured JSON with the rewritten command
5. Kimi executes the rewritten command (e.g., `rtk git status`)

## JSON Format

**Input** (stdin):
```json
{
  "tool_name": "Shell",
  "tool_input": { "command": "git status" }
}
```

**Output** (stdout, when rewritten):
```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "allow",
    "permissionDecisionReason": "RTK auto-rewrite",
    "updatedInput": { "command": "rtk git status" }
  }
}
```

**No rewrite**: empty stdout (exit 0) — Kimi runs the original command unchanged.

## Files

| File | Purpose |
|------|---------|
| `~/.kimi/config.toml` | Hook registration (TOML `[[hooks]]` array) |
| `~/.kimi/KIMI.md` | Slim RTK awareness instructions |

## Uninstall

```bash
rtk init -g --kimi --uninstall
```
