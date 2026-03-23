#!/usr/bin/env bash

if ! command -v jq &>/dev/null; then
  exit 0
fi

if ! command -v rtk &>/dev/null; then
  exit 0
fi

RTK_LOG_DIR="${RTK_LOG_DIR:-$HOME/.local/share/rtk/logs}"
mkdir -p "$RTK_LOG_DIR"
LOG_FILE="$RTK_LOG_DIR/mcp-filter.log"

INPUT=$(cat)

TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name // empty' 2>/dev/null)

if [ -z "$TOOL_NAME" ]; then
  exit 0
fi

case "$TOOL_NAME" in
  mcp__*) ;;
  *) exit 0 ;;
esac

RESULT=$(echo "$INPUT" | rtk filter-mcp-output 2>>"$LOG_FILE")
EXIT_CODE=$?

if [ $EXIT_CODE -ne 0 ]; then
  echo "[$(date -u +%Y-%m-%dT%H:%M:%SZ)] filter-mcp-output failed (exit $EXIT_CODE) for tool: $TOOL_NAME" >> "$LOG_FILE"
  exit 0
fi

if [ -z "$RESULT" ]; then
  exit 0
fi

echo "$RESULT"
