#!/usr/bin/env bash
# RTK PreToolUse Hook for Charmbracelet Crush
set -e

# 1. Fail-open if RTK is missing from PATH
if ! command -v rtk &> /dev/null; then
  echo "{}"
  exit 0
fi

# 2. Extract the raw command from Crush's injected environment variable
RAW_CMD="$CRUSH_TOOL_INPUT_COMMAND"
if [ -z "$RAW_CMD" ]; then
    echo "{}"
    exit 0
fi

# 3. Rewrite using RTK
# We use jq to safely escape the rewritten command for the JSON payload
REWRITTEN=$(rtk rewrite "$RAW_CMD")
ESCAPED_CMD=$(jq -n --arg c "$REWRITTEN" '$c')

# 4. Emit the updated_input JSON contract required by Crush
cat <<EOF
{
  "updated_input": {
    "command": $ESCAPED_CMD
  }
}
EOF
