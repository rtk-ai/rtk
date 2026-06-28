#!/usr/bin/env bash
# rtk-hook-version: 2
# RTK Cursor Agent hook — rewrites shell commands to use rtk for token savings.
# Works with both Cursor editor and cursor-cli (they share ~/.cursor/hooks.json).
# Cursor preToolUse hook format: receives JSON on stdin, returns JSON on stdout.
# Requires: rtk >= 0.23.0, jq
#
# This is a thin delegating hook: all rewrite logic lives in `rtk rewrite`,
# which is the single source of truth (src/discover/registry.rs).
# To add or change rewrite rules, edit the Rust registry — not this file.

if ! command -v jq &>/dev/null; then
  echo "[rtk] WARNING: jq is not installed. Hook cannot rewrite commands. Install jq: https://jqlang.github.io/jq/download/" >&2
  exit 0
fi

if ! command -v rtk &>/dev/null; then
  echo "[rtk] WARNING: rtk is not installed or not in PATH. Hook cannot rewrite commands. Install: https://github.com/rtk-ai/rtk#installation" >&2
  exit 0
fi

# Version guard: rtk rewrite was added in 0.23.0.
RTK_VERSION=$(rtk --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)
if [ -n "$RTK_VERSION" ]; then
  MAJOR=$(echo "$RTK_VERSION" | cut -d. -f1)
  MINOR=$(echo "$RTK_VERSION" | cut -d. -f2)
  if [ "$MAJOR" -eq 0 ] && [ "$MINOR" -lt 23 ]; then
    echo "[rtk] WARNING: rtk $RTK_VERSION is too old (need >= 0.23.0). Upgrade: cargo install rtk" >&2
    exit 0
  fi
fi

INPUT=$(cat)
CMD=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

if [ -z "$CMD" ]; then
  echo '{}'
  exit 0
fi

# Delegate all rewrite logic to the Rust binary.
REWRITTEN=$(rtk rewrite "$CMD" 2>/dev/null)
EXIT_CODE=$?

# 1 = No RTK equivalent, 2 = Deny rule matched
if [ "$EXIT_CODE" -eq 1 ] || [ "$EXIT_CODE" -eq 2 ]; then
  echo '{}'
  exit 0
fi

# No change — nothing to do.
if [ -z "$REWRITTEN" ] || [ "$CMD" = "$REWRITTEN" ]; then
  echo '{}'
  exit 0
fi

# Exit code 0 (Allow) or 3 (Ask/Default).
# Cursor's hook model currently requires "permission": "allow" to auto-run the rewritten command.
jq -n --arg cmd "$REWRITTEN" '{
  "permission": "allow",
  "updated_input": { "command": $cmd }
}'
