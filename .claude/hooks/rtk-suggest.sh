#!/usr/bin/env bash
# RTK suggest hook for Claude Code PreToolUse:Bash
# Delegates to `rtk suggest` to check if a command has an RTK equivalent.
# Outputs JSON with systemMessage to inform Claude Code without modifying execution.
#
# This hook is intentionally thin — all rewrite logic lives in
# src/discover/registry.rs (single source of truth).

set -euo pipefail

INPUT=$(cat)
CMD=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

# No command → pass through
[ -z "$CMD" ] && exit 0

# Already using rtk → skip
case "$CMD" in
  rtk\ *|*/rtk\ *) exit 0 ;;
esac

# Heredocs → skip (not safe to rewrite)
case "$CMD" in
  *'<<'*) exit 0 ;;
esac

# Ask rtk for a suggestion (exits 1 if no equivalent)
SUGGESTION=$(rtk suggest "$CMD" 2>/dev/null) || exit 0

# Emit suggestion as system message
jq -n \
  --arg suggestion "$SUGGESTION" \
  '{
    "hookSpecificOutput": {
      "hookEventName": "PreToolUse",
      "permissionDecision": "allow",
      "systemMessage": ("RTK available: `" + $suggestion + "` (60-90% token savings)")
    }
  }'
