#!/usr/bin/env bash
# rtk-hook-version: 1
# RTK Mistral Vibe hook — rewrites bash commands to use rtk for token savings.
# Requires: rtk >= 0.23.0, jq, Mistral Vibe >= 2.15.0
#
# This is a thin delegating hook: all rewrite logic lives in `rtk rewrite`,
# which is the single source of truth (src/discover/registry.rs).
# To add or change rewrite rules, edit the Rust registry — not this file.
#
# Exit code protocol:
#   0 + empty stdout  Pass through unchanged
#   0 + JSON stdout   Structured response (rewrite/deny)
#   Any non-zero       Treated as hook failure (fail-open by Vibe)
#
# Vibe before_tool hook contract:
#   - Receives JSON on stdin with tool_name, tool_input, etc.
#   - Returns JSON on stdout with decision and hook_specific_output
#   - Exit 0 means success, non-zero means hook failure

set -euo pipefail

# Fail-open: if jq is missing, warn and exit 0 (pass through)
if ! command -v jq &>/dev/null; then
  echo "[rtk] WARNING: jq is not installed. Hook cannot rewrite commands. Install jq: https://jqlang.github.io/jq/download/" >&2
  exit 0
fi

# Fail-open: if rtk is missing, warn and exit 0 (pass through)
if ! command -v rtk &>/dev/null; then
  echo "[rtk] WARNING: rtk is not installed or not in PATH. Hook cannot rewrite commands. Install: https://github.com/rtk-ai/rtk#installation" >&2
  exit 0
fi

# Version guard: rtk rewrite was added in 0.23.0.
# Cache the version check to avoid spawning multiple processes on every hook call.
CACHE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}"
CACHE_FILE="$CACHE_DIR/rtk-hook-vibe-version-ok"
if [ ! -f "$CACHE_FILE" ]; then
  RTK_VERSION_RAW=$(rtk --version 2>/dev/null || true)
  RTK_VERSION=${RTK_VERSION_RAW#rtk }
  RTK_VERSION=${RTK_VERSION%% *}
  if [ -n "$RTK_VERSION" ]; then
    IFS=. read -r MAJOR MINOR PATCH <<<"$RTK_VERSION"
    # Require >= 0.23.0
    if [ "$MAJOR" -eq 0 ] && [ "$MINOR" -lt 23 ]; then
      echo "[rtk] WARNING: rtk $RTK_VERSION is too old (need >= 0.23.0). Upgrade: cargo install --git https://github.com/rtk-ai/rtk" >&2
      exit 0
    fi
  fi
  mkdir -p "$CACHE_DIR" 2>/dev/null || true
  touch "$CACHE_FILE" 2>/dev/null || true
fi

# Read all stdin
INPUT=$(cat)

# Extract tool_name and tool_input.command
TOOL_NAME=$(jq -r '.tool_name // empty' <<<"$INPUT")
TOOL_INPUT=$(jq -c '.tool_input // empty' <<<"$INPUT")

# Only process bash and run_shell_command tools
# Vibe uses "bash" for the bash tool and "run_shell_command" for shell commands
if [ "$TOOL_NAME" != "bash" ] && [ "$TOOL_NAME" != "run_shell_command" ]; then
  exit 0
fi

# Extract command from tool_input
CMD=$(jq -r '.command // empty' <<<"$TOOL_INPUT")

if [ -z "$CMD" ]; then
  exit 0
fi

# Check for RTK_DISABLED override
if [ "${RTK_DISABLED:-0}" = "1" ]; then
  exit 0
fi

# Delegate all rewrite logic to the Rust binary.
# rtk rewrite exits:
#   0 - rewrite found
#   1 - no RTK equivalent (pass through)
#   2 - deny rule matched (not used by RTK, pass through)
#   3 - ask rule matched (rewrite but ask user)
REWRITTEN=$(rtk rewrite "$CMD" 2>/dev/null || true)
EXIT_CODE=$?

# Handle exit codes
case $EXIT_CODE in
  0)
    # Rewrite found - use it
    ;;
  1)
    # No RTK equivalent - pass through
    exit 0
    ;;
  2)
    # Deny rule matched - pass through (RTK doesn't use deny rules)
    exit 0
    ;;
  3)
    # Ask rule matched - rewrite but we'll allow it (Vibe handles permission separately)
    ;;
  *)
    # Unexpected error - pass through
    exit 0
    ;;
esac

# If rewritten is empty or same as original, pass through
if [ -z "$REWRITTEN" ] || [ "$REWRITTEN" = "$CMD" ]; then
  exit 0
fi

# Return JSON response with rewritten command
# Vibe expects: decision (allow/deny), hook_specific_output.tool_input
jq -n --arg cmd "$REWRITTEN" '{
  "decision": "allow",
  "hook_specific_output": {
    "tool_input": { "command": $cmd }
  }
}'
