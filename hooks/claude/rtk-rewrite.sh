#!/usr/bin/env bash
# rtk-hook-version: 3
# RTK Claude Code hook — rewrites commands to use rtk for token savings.
# Requires: rtk >= 0.23.0, jq
#
# This is a thin delegating hook: all rewrite logic lives in `rtk rewrite`,
# which is the single source of truth (src/discover/registry.rs).
# To add or change rewrite rules, edit the Rust registry — not this file.
#
# Exit code protocol for `rtk rewrite`:
#   0 + stdout  Rewrite found, no deny/ask rule matched → auto-allow
#   1           No RTK equivalent → pass through unchanged
#   2           Deny rule matched → pass through (Claude Code native deny handles it)
#   3 + stdout  Ask rule matched → rewrite but let Claude Code prompt the user

if ! command -v jq &>/dev/null; then
  echo "[rtk] WARNING: jq is not installed. Hook cannot rewrite commands. Install jq: https://jqlang.github.io/jq/download/" >&2
  exit 0
fi

if ! command -v rtk &>/dev/null; then
  echo "[rtk] WARNING: rtk is not installed or not in PATH. Hook cannot rewrite commands. Install: https://github.com/rtk-ai/rtk#installation" >&2
  exit 0
fi

# Version guard: rtk rewrite was added in 0.23.0.
# Older binaries: warn once and exit cleanly (no silent failure).
# Cache the version check to avoid spawning multiple processes on every hook call.
CACHE_DIR=${XDG_CACHE_HOME:-$HOME/.cache}
CACHE_FILE="$CACHE_DIR/rtk-hook-version-ok"
if [ ! -f "$CACHE_FILE" ]; then
  RTK_VERSION_RAW=$(rtk --version 2>/dev/null)
  RTK_VERSION=${RTK_VERSION_RAW#rtk }
  RTK_VERSION=${RTK_VERSION%% *}
  if [ -n "$RTK_VERSION" ]; then
    IFS=. read -r MAJOR MINOR PATCH <<<"$RTK_VERSION"
    # Require >= 0.23.0
    if [ "$MAJOR" -eq 0 ] && [ "$MINOR" -lt 23 ]; then
      echo "[rtk] WARNING: rtk $RTK_VERSION is too old (need >= 0.23.0). Upgrade: cargo install rtk" >&2
      exit 0
    fi
  fi
  mkdir -p "$CACHE_DIR" 2>/dev/null
  touch "$CACHE_FILE" 2>/dev/null
fi

INPUT=$(cat)
CMD=$(jq -r '.tool_input.command // empty' <<<"$INPUT")

if [ -z "$CMD" ]; then
  exit 0
fi

audit_log() {
  [ "${RTK_HOOK_AUDIT:-}" = "1" ] || return 0
  local action="$1"
  local original="$2"
  local rewritten="$3"
  local audit_dir="${RTK_AUDIT_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/rtk}"
  mkdir -p "$audit_dir" 2>/dev/null || return 0
  original=${original//\\/\\\\}
  original=${original//|/\\|}
  original=${original//$'\n'/\\n}
  original=${original//$'\r'/\\r}
  rewritten=${rewritten//\\/\\\\}
  rewritten=${rewritten//|/\\|}
  rewritten=${rewritten//$'\n'/\\n}
  rewritten=${rewritten//$'\r'/\\r}
  printf '%s | %s | %s | %s\n' \
    "$(date '+%Y-%m-%dT%H:%M:%S')" "$action" "$original" "$rewritten" \
    >>"$audit_dir/hook-audit.log" 2>/dev/null || true
}

# Delegate all rewrite + permission logic to the Rust binary.
REWRITTEN=$(rtk rewrite "$CMD" 2>/dev/null)
EXIT_CODE=$?

# Permission policy can return 3 (ask) even when an already-RTK command is
# unchanged. Identity is the authoritative signal that there is no rewrite.
if { [ "$EXIT_CODE" -eq 0 ] || [ "$EXIT_CODE" -eq 3 ]; } && [ "$CMD" = "$REWRITTEN" ]; then
  audit_log "skip:already_rtk" "$CMD" "$REWRITTEN"
  exit 0
fi

case $EXIT_CODE in
  0)
    # Rewrite found, no permission rules matched — safe to auto-allow.
    audit_log "rewrite" "$CMD" "$REWRITTEN"
    ;;
  1)
    # No RTK equivalent — pass through unchanged.
    if [[ "$CMD" == *"<<"* ]]; then
      audit_log "skip:heredoc" "$CMD" "$CMD"
    else
      audit_log "skip:no_match" "$CMD" "$CMD"
    fi
    exit 0
    ;;
  2)
    # Deny rule matched — let Claude Code's native deny rule handle it.
    audit_log "skip:deny" "$CMD" "$CMD"
    exit 0
    ;;
  3)
    # Ask rule matched — rewrite the command but do NOT auto-allow so that
    # Claude Code prompts the user for confirmation.
    audit_log "rewrite" "$CMD" "$REWRITTEN"
    ;;
  *)
    audit_log "skip:error" "$CMD" "$CMD"
    exit 0
    ;;
esac

if [ "$EXIT_CODE" -eq 3 ]; then
  # Ask: rewrite the command, omit permissionDecision so Claude Code prompts.
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
