# RTK + Cursor Integration Guide

This guide explains how to use **RTK (Rust Token Killer)** with **Cursor** using hooks.

Cursor does not support RTK’s native hook system (designed for Claude Code), so a custom hook is required.

---

## Pre-Check (REQUIRED)

Verify RTK is correctly installed:

```bash
rtk --version
rtk gain
```

Expected:
- `rtk gain` shows token savings stats
- NOT "command not found"

If `rtk gain` fails, fix your RTK installation before continuing.

---

## Setup Overview

Cursor integration requires:

1. A hook to rewrite shell commands
2. A writable RTK database path
3. (Optional) Sandbox authorization

---

## 1. Create Hook Directory

```bash
mkdir -p .cursor/hooks
```

---

## 2. Register Hook

File: `.cursor/hooks.json`

```json
{
  "version": 1,
  "hooks": {
    "preToolUse": [
      {
        "matcher": "Shell",
        "command": ".cursor/hooks/rtk-rewrite.sh"
      }
    ]
  }
}
```

---

## 3. Create Rewrite Hook

File: `.cursor/hooks/rtk-rewrite.sh`

```bash
#!/usr/bin/env bash
set -euo pipefail

# Quietly no-op if dependencies are missing
command -v jq >/dev/null 2>&1 || exit 0
command -v rtk >/dev/null 2>&1 || exit 0

INPUT=$(cat)
CMD=$(printf '%s' "$INPUT" | jq -r '.tool_input.command // empty')

# Nothing to rewrite
[ -n "$CMD" ] || exit 0

# Ask RTK to rewrite if it knows how
REWRITTEN=$(rtk rewrite "$CMD" 2>/dev/null) || exit 0

# No change -> no hook output
[ "$REWRITTEN" != "$CMD" ] || exit 0

# Preserve the original Shell tool input, only replace command
UPDATED_INPUT=$(printf '%s' "$INPUT" | jq -c --arg cmd "$REWRITTEN" '
  .tool_input
  | .command = $cmd
')

jq -n \
  --argjson updated "$UPDATED_INPUT" \
  '{
    "permission": "allow",
    "updated_input": $updated
  }'

```

---

## 4. Make Hook Executable

```bash
chmod +x .cursor/hooks/rtk-rewrite.sh
```

---

## 5. Configure RTK Database (REQUIRED)

RTK requires a writable SQLite database for tracking.

Create a global directory:

```bash
mkdir -p "$HOME/.rtk"
```

Set the database path:

```bash
export RTK_DB_PATH="$HOME/.rtk/tracking.db"
```

Add this to your shell config:

- macOS: `~/.zshrc`
- Linux: `~/.bashrc` or `~/.zshrc`

Reload:

```bash
source ~/.zshrc   # or ~/.bashrc
```

Verify:

```bash
echo "$RTK_DB_PATH"
```

---

## 6. Test Integration

Ask Cursor agent:

> run `rtk gain`

Expected:
- command executes successfully
- token stats are displayed

If this works, setup is complete.

---

## 7. Sandbox Authorization (if needed)

If you see:

```
unable to open database file
```

Cursor sandbox is blocking access.

Create global sandbox config:

```bash
mkdir -p ~/.cursor
touch ~/.cursor/sandbox.json
```

Edit `~/.cursor/sandbox.json`:

```json
{
  "type": "workspace_readwrite",
  "additionalReadwritePaths": [
    "/Users/your-user/.rtk"
  ]
}
```

Use your actual home directory (no `~`).

Restart Cursor after changes.

---

## Verification

```bash
rtk rewrite "git status"
```

Then ask Cursor:

> run `rtk gain`

---

## Expected Behavior

- Cursor runs shell commands
- Hook rewrites commands via `rtk rewrite`
- RTK executes optimized commands
- Output is reduced before reaching the model

---

## Notes

- Do NOT store the RTK database inside `.cursor/`
- Ensure `RTK_DB_PATH` is available to Cursor (use `~/.zprofile` if needed)
- Hooks fail open if `rtk` or `jq` is unavailable
- This setup replaces `rtk init -g` (not supported in Cursor)

---


## Troubleshooting

### `rtk gain` works locally but fails in Cursor

Cause:
- sandbox restriction

Fix:
- allow RTK DB path in `sandbox.json`
- restart Cursor


---

### Hook not triggered

Check:

```bash
rtk rewrite "git status"
```

If working:
- verify `.cursor/hooks.json` path
- ensure script is executable
