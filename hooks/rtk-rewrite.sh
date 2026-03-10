#!/usr/bin/env bash
# rtk-hook-version: 3
# RTK Claude Code hook — rewrites commands to use rtk for token savings.
# Requires: rtk >= 0.29.0, jq
#
# This is a thin delegating hook: ALL logic (rewrite rules, permission
# decisions, config loading) lives in `rtk rewrite --hook-json`.
# To change behavior, edit ~/.config/rtk/config.toml — not this file.

if ! command -v jq &>/dev/null; then
  echo "[rtk] WARNING: jq is not installed. Hook cannot rewrite commands. Install jq: https://jqlang.github.io/jq/download/" >&2
  exit 0
fi

if ! command -v rtk &>/dev/null; then
  echo "[rtk] WARNING: rtk is not installed or not in PATH. Hook cannot rewrite commands. Install: https://github.com/rtk-ai/rtk#installation" >&2
  exit 0
fi

# Version guard: --hook-json was added in 0.29.0.
RTK_VERSION=$(rtk --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)
if [ -n "$RTK_VERSION" ]; then
  MAJOR=$(echo "$RTK_VERSION" | cut -d. -f1)
  MINOR=$(echo "$RTK_VERSION" | cut -d. -f2)
  if [ "$MAJOR" -eq 0 ] && [ "$MINOR" -lt 29 ]; then
    echo "[rtk] WARNING: rtk $RTK_VERSION is too old (need >= 0.29.0). Upgrade: cargo install rtk" >&2
    exit 0
  fi
fi

INPUT=$(cat)
CMD=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

if [ -z "$CMD" ]; then
  exit 0
fi

# Delegate everything to Rust: rewrite decision, config, permission, JSON output.
# Empty output = no rewrite (pass through silently).
RESULT=$(rtk rewrite --hook-json "$CMD" 2>/dev/null) || exit 0

if [ -n "$RESULT" ]; then
  echo "$RESULT"
fi
