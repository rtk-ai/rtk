#!/usr/bin/env bash
# rtk-hook-version: 1
# RTK Augment Code (Auggie) hook — rewrites commands to use rtk for token savings.
# Requires: rtk >= 0.28.0, jq
#
# This is a thin delegating hook: all rewrite logic lives in `rtk rewrite`,
# which is the single source of truth (src/discover/registry.rs).
# To add or change rewrite rules, edit the Rust registry — not this file.
#
# The Auggie PreToolUse payload matches Claude Code's format:
#   Input:  { "tool_name": "launch-process", "tool_input": { "command": "..." } }
#   Output: { "hookSpecificOutput": { "hookEventName": "PreToolUse", ... } }
#
# Exit code protocol for `rtk rewrite`:
#   0 + stdout  Rewrite found, no deny/ask rule matched → auto-allow
#   1           No RTK equivalent → pass through unchanged
#   2           Deny rule matched → pass through
#   3 + stdout  Ask rule matched → rewrite but let agent prompt the user

set -euo pipefail

# ── Version guard ──────────────────────────────────────────
MIN_VERSION="0.28.0"
RTK_VERSION=$(rtk --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' || echo "0.0.0")

version_lt() {
  [ "$(printf '%s\n' "$1" "$2" | sort -V | head -n1)" = "$1" ] && [ "$1" != "$2" ]
}

if ! command -v rtk &>/dev/null; then
  exit 0
fi

if version_lt "$RTK_VERSION" "$MIN_VERSION"; then
  echo "rtk $RTK_VERSION < $MIN_VERSION, skipping hook" >&2
  exit 0
fi

# ── Main logic ─────────────────────────────────────────────

INPUT=$(cat)
CMD=$(jq -r '.tool_input.command // empty' <<<"$INPUT")

if [ -z "$CMD" ]; then
  exit 0
fi

# Delegate all rewrite + permission logic to the Rust binary.
REWRITTEN=$(rtk rewrite "$CMD" 2>/dev/null)
EXIT_CODE=$?

case $EXIT_CODE in
  0)
    [ "$CMD" = "$REWRITTEN" ] && exit 0
    ;;
  1)
    exit 0
    ;;
  2)
    exit 0
    ;;
  3)
    ;;
  *)
    exit 0
    ;;
esac

if [ "$EXIT_CODE" -eq 3 ]; then
  # Ask: rewrite the command, omit permissionDecision so agent prompts.
  jq -c --arg cmd "$REWRITTEN" \
    '.tool_input.command = $cmd | {
      "hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "updatedInput": .tool_input
      }
    }' <<<"$INPUT"
else
  # Allow: rewrite the command and auto-allow.
  jq -c --arg cmd "$REWRITTEN" \
    '.tool_input.command = $cmd | {
      "hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "allow",
        "permissionDecisionReason": "RTK auto-rewrite",
        "updatedInput": .tool_input
      }
    }' <<<"$INPUT"
fi
