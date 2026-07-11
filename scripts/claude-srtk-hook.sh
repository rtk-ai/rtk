#!/bin/bash
# Claude Code PreToolUse hook: rewrite Bash commands to their srtk equivalent.
# srtk = rtk build with PII redaction (slice-ravichopra/rtk, feat/pii-redaction).
# Only rewrites the command text — permission checks still apply to the
# rewritten command as usual (no permissionDecision is emitted).
# Exits 0 with no output → command runs unchanged.
#
# Installed to ~/.claude/hooks/rtk-rewrite.sh by scripts/install-srtk.sh.
# Requires jq (brew install jq).

SRTK="$HOME/.local/bin/srtk"
[ -x "$SRTK" ] || exit 0

JQ="$(command -v jq)"
[ -n "$JQ" ] || exit 0

INPUT=$(cat)
CMD=$(printf '%s' "$INPUT" | "$JQ" -r '.tool_input.command // empty' 2>/dev/null)
[ -n "$CMD" ] || exit 0

# Never rewrite commands already using rtk/srtk.
case "$CMD" in
  srtk\ *|rtk\ *|*"/srtk "*|*"/rtk "*) exit 0 ;;
esac

# srtk rewrite exit codes: 0 = rewritten (allow-listed), 3 = rewritten (no
# allow rule yet — Claude will ask as usual), 1 = no equivalent, 2 = deny.
REWRITTEN=$("$SRTK" rewrite "$CMD" 2>/dev/null)
rc=$?
{ [ "$rc" -eq 0 ] || [ "$rc" -eq 3 ]; } || exit 0
[ -n "$REWRITTEN" ] || exit 0

# srtk emits "rtk ..." — force the srtk binary name.
REWRITTEN="srtk ${REWRITTEN#rtk }"

printf '{"hookSpecificOutput":{"hookEventName":"PreToolUse","updatedInput":{"command":%s}}}' \
  "$(printf '%s' "$REWRITTEN" | "$JQ" -Rs .)"
