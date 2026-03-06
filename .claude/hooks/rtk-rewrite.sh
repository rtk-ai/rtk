#!/bin/bash
# RTK auto-rewrite hook for Claude Code PreToolUse:Bash
# Transparently rewrites raw commands to their RTK equivalents.
# Uses `rtk rewrite` as single source of truth — no duplicate mapping logic here.
#
# To add support for new commands, update src/discover/registry.rs (PATTERNS + RULES).

# --- Audit logging (opt-in via RTK_HOOK_AUDIT=1) ---
_rtk_audit_log() {
  if [ "${RTK_HOOK_AUDIT:-0}" != "1" ]; then return; fi
  local action="$1" original="$2" rewritten="${3:--}"
  local dir="${RTK_AUDIT_DIR:-${HOME}/.local/share/rtk}"
  mkdir -p "$dir"
  printf '%s | %s | %s | %s\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$action" "$original" "$rewritten" \
    >> "${dir}/hook-audit.log"
}

# Guards: skip silently if dependencies missing
if ! command -v rtk &>/dev/null || ! command -v jq &>/dev/null; then
  _rtk_audit_log "skip:no_deps" "-"
  exit 0
fi

set -euo pipefail

INPUT=$(cat)
CMD=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

if [ -z "$CMD" ]; then
  _rtk_audit_log "skip:empty" "-"
  exit 0
fi

# Skip heredocs (rtk rewrite also skips them, but bail early)
case "$CMD" in
  *'<<'*) _rtk_audit_log "skip:heredoc" "$CMD"; exit 0 ;;
esac

# Rewrite via rtk — single source of truth for all command mappings.
# Exit 1 = no RTK equivalent, pass through unchanged.
# Exit 0 = rewritten command (or already RTK, identical output).
REWRITTEN=$(rtk rewrite "$CMD" 2>/dev/null) || {
  _rtk_audit_log "skip:no_match" "$CMD"
  exit 0
}

# If output is identical, command was already using RTK — nothing to do.
if [ "$CMD" = "$REWRITTEN" ]; then
  _rtk_audit_log "skip:already_rtk" "$CMD"
  exit 0
fi

_rtk_audit_log "rewrite" "$CMD" "$REWRITTEN"

# Build the updated tool_input with all original fields preserved, only command changed.
ORIGINAL_INPUT=$(echo "$INPUT" | jq -c '.tool_input')
UPDATED_INPUT=$(echo "$ORIGINAL_INPUT" | jq --arg cmd "$REWRITTEN" '.command = $cmd')

# Output the rewrite instruction in Claude Code hook format.
jq -n \
  --argjson updated "$UPDATED_INPUT" \
  '{
    "hookSpecificOutput": {
      "hookEventName": "PreToolUse",
      "permissionDecision": "allow",
      "permissionDecisionReason": "RTK auto-rewrite",
      "updatedInput": $updated
    }
  }'
