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
#
# Exit code protocol for `rtk rewrite`:
#   0 + stdout  Rewrite found, no deny/ask rule matched → auto-allow
#   1           No RTK equivalent → pass through unchanged
#   2           Deny rule matched → pass through
#   3 + stdout  Ask rule matched → rewrite (Cursor has no ask model, so auto-allow)

if ! command -v jq &>/dev/null; then
  echo "[rtk] WARNING: jq is not installed. Hook cannot rewrite commands. Install jq: https://jqlang.github.io/jq/download/" >&2
  exit 0
fi

if ! command -v rtk &>/dev/null; then
  echo "[rtk] WARNING: rtk is not installed or not in PATH. Hook cannot rewrite commands. Install: https://github.com/rtk-ai/rtk#installation" >&2
  exit 0
fi

# Version guard: rtk rewrite was added in 0.23.0.
# Cache the version check to avoid spawning `rtk --version` on every hook call.
CACHE_DIR=${XDG_CACHE_HOME:-$HOME/.cache}
CACHE_FILE="$CACHE_DIR/rtk-cursor-hook-version-ok"
if [ ! -f "$CACHE_FILE" ]; then
  RTK_VERSION=$(rtk --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)
  if [ -n "$RTK_VERSION" ]; then
    MAJOR=$(echo "$RTK_VERSION" | cut -d. -f1)
    MINOR=$(echo "$RTK_VERSION" | cut -d. -f2)
    if [ "$MAJOR" -eq 0 ] && [ "$MINOR" -lt 23 ]; then
      echo "[rtk] WARNING: rtk $RTK_VERSION is too old (need >= 0.23.0). Upgrade: cargo install rtk" >&2
      exit 0
    fi
  fi
  mkdir -p "$CACHE_DIR" 2>/dev/null
  touch "$CACHE_FILE" 2>/dev/null
fi

INPUT=$(cat)
CMD=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

if [ -z "$CMD" ]; then
  echo '{}'
  exit 0
fi

# Delegate all rewrite logic to the Rust binary.
# Exit code contract: 0 = rewrite (allow), 1 = no match, 2 = deny, 3 = rewrite (ask).
# Cursor has no ask/deny model, so both 0 and 3 produce an auto-allow rewrite.
REWRITTEN=$(rtk rewrite "$CMD" 2>/dev/null)
RC=$?

case $RC in
  0|3)
    if [ "$CMD" = "$REWRITTEN" ]; then
      echo '{}'
      exit 0
    fi
    ;;
  *)
    echo '{}'
    exit 0
    ;;
esac

jq -n --arg cmd "$REWRITTEN" '{
  "permission": "allow",
  "updated_input": { "command": $cmd }
}'
