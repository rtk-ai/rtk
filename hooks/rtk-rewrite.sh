#!/usr/bin/env bash
# rtk-hook-version: 2
# RTK Claude Code hook — rewrites commands to use rtk for token savings.
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
# Older binaries: warn once and exit cleanly (no silent failure).
RTK_VERSION=$(rtk --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)
if [ -n "$RTK_VERSION" ]; then
  MAJOR=$(echo "$RTK_VERSION" | cut -d. -f1)
  MINOR=$(echo "$RTK_VERSION" | cut -d. -f2)
  # Require >= 0.23.0
  if [ "$MAJOR" -eq 0 ] && [ "$MINOR" -lt 23 ]; then
    echo "[rtk] WARNING: rtk $RTK_VERSION is too old (need >= 0.23.0). Upgrade: cargo install rtk" >&2
    exit 0
  fi
fi

INPUT=$(cat)
CMD=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

# RAII temp file cleanup — trap handles SIGTERM/SIGINT/EXIT to prevent leaks.
TEMP_FILES=()
cleanup() { for f in "${TEMP_FILES[@]}"; do [ -n "$f" ] && rm -f "$f"; done; }
trap cleanup EXIT INT TERM

# Helper: check if response JSON contains a deny decision (CC + Gemini dual format)
# CC format:     hookSpecificOutput.permissionDecision == "deny"
# Gemini format: decision == "deny"  (top-level; Gemini handlers use this today)
# jq -e exits 0 for truthy, 1 for false/null. >/dev/null suppresses output.
has_json_deny() {
  local f="$1"; [ -s "$f" ] || return 1
  jq -e '(.hookSpecificOutput.permissionDecision == "deny") or (.decision == "deny")' \
    "$f" >/dev/null 2>&1
}

# Helper: check if response JSON contains updatedInput (rewrite path)
has_updated_input() {
  local f="$1"; [ -s "$f" ] || return 1
  jq -e '.hookSpecificOutput.updatedInput != null' "$f" >/dev/null 2>&1
}

# Handler tracking arrays — declared here so BEGIN_RTK_BASH_HANDLERS can append to them.
HANDLER_PIDS=()
HANDLER_OUTS=()
HANDLER_ERRS=()

if [ -z "$CMD" ]; then
  exit 0
fi

# Delegate all rewrite logic to the Rust binary.
# rtk rewrite exits 1 when there's no rewrite — hook passes through silently.
REWRITTEN=$(rtk rewrite "$CMD" 2>/dev/null) || exit 0


# No change — nothing to do.
if [ "$CMD" = "$REWRITTEN" ]; then
  exit 0
fi

# === BEGIN_RTK_BASH_HANDLERS (managed by rtk init — parallel launch) ===
# rtk init adds handler entries here; each entry must:
#   1. Allocate and register temp files:  _HN_OUT=$(mktemp); TEMP_FILES+=("$_HN_OUT")
#   2. Launch in background:              printf '%s' "$INPUT" | handler >"$_HN_OUT" 2>"$_HN_ERR" &
#   3. Register PID/OUT/ERR:              HANDLER_PIDS+=($!); HANDLER_OUTS+=("$_HN_OUT"); HANDLER_ERRS+=("$_HN_ERR")
# === END_RTK_BASH_HANDLERS ===

# Build RTK rewrite JSON into temp file (if applicable)
RTK_OUT=$(mktemp 2>/dev/null) || RTK_OUT=""
[ -n "$RTK_OUT" ] && TEMP_FILES+=("$RTK_OUT")
if [ -n "$REWRITTEN" ] && [ -n "$RTK_OUT" ]; then
  # Build the updated tool_input with all original fields preserved, only command changed
  ORIGINAL_INPUT=$(echo "$INPUT" | jq -c '.tool_input')
  UPDATED_INPUT=$(echo "$ORIGINAL_INPUT" | jq --arg cmd "$REWRITTEN" '.command = $cmd')
  jq -n \
    --argjson updated "$UPDATED_INPUT" \
    '{
      "hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "allow",
        "permissionDecisionReason": "RTK auto-rewrite",
        "updatedInput": $updated
      }
    }' >"$RTK_OUT" 2>/dev/null
fi

# COLLECT PHASE: wait for all registered handlers (launched in BEGIN_RTK_BASH_HANDLERS above)
# ALL handlers run — no short-circuit before this point.
HANDLER_EXITS=()
for pid in "${HANDLER_PIDS[@]}"; do
  wait "$pid" 2>/dev/null
  HANDLER_EXITS+=($?)
done

# MERGE PHASE: deny wins over rewrite; rewrite wins over pass-through.
# Block detection is ONLY here — not in launch phase (all handlers always run).
BLOCK_OUT=""
BLOCK_ERR=""
for i in "${!HANDLER_PIDS[@]}"; do
  if [ "${HANDLER_EXITS[$i]}" -eq 2 ] || has_json_deny "${HANDLER_OUTS[$i]}"; then
    BLOCK_OUT="${HANDLER_OUTS[$i]}"
    BLOCK_ERR="${HANDLER_ERRS[$i]}"
    break  # First block wins; all handlers already ran above
  fi
done

# Block propagation: exit 2 (reliable; bug #4669 workaround for CC, also valid when fixed)
# trap cleanup handles temp file removal on exit
if [ -n "$BLOCK_OUT" ]; then
  [ -s "$BLOCK_OUT" ] && cat "$BLOCK_OUT"
  [ -n "$BLOCK_ERR" ] && [ -s "$BLOCK_ERR" ] && cat "$BLOCK_ERR" >&2
  exit 2
fi

# RTK rewrite: apply if RTK produced one and no handler blocked
if has_updated_input "$RTK_OUT"; then
  cat "$RTK_OUT"
  exit 0
fi

# No opinions from any handler — pass-through (no stdout)
# (trap cleanup runs automatically on exit)
exit 0
