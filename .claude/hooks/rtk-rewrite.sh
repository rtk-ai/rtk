#!/bin/bash
# RTK auto-rewrite hook for Claude Code PreToolUse:Bash
# Transparently rewrites raw commands to their rtk equivalents.
# Outputs JSON with updatedInput to modify the command before execution.

set -euo pipefail

INPUT=$(cat)
CMD=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

if [ -z "$CMD" ]; then
  exit 0
fi

# Extract the first meaningful command (before pipes, &&, etc.)
# We only rewrite if the FIRST command in a chain matches.
FIRST_CMD="$CMD"

# Skip if already using rtk
case "$FIRST_CMD" in
  rtk\ *|*/rtk\ *) exit 0 ;;
esac

# Skip commands with heredocs, variable assignments as the whole command, etc.
case "$FIRST_CMD" in
  *'<<'*) exit 0 ;;
esac

REWRITTEN=""

# --- Git commands ---
if echo "$FIRST_CMD" | grep -qE '^git\s+status(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^git status/rtk git status/')
elif echo "$FIRST_CMD" | grep -qE '^git\s+diff(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^git diff/rtk git diff/')
elif echo "$FIRST_CMD" | grep -qE '^git\s+log(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^git log/rtk git log/')
elif echo "$FIRST_CMD" | grep -qE '^git\s+add(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^git add/rtk git add/')
elif echo "$FIRST_CMD" | grep -qE '^git\s+commit(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^git commit/rtk git commit/')
elif echo "$FIRST_CMD" | grep -qE '^git\s+push(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^git push/rtk git push/')
elif echo "$FIRST_CMD" | grep -qE '^git\s+pull(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^git pull/rtk git pull/')
elif echo "$FIRST_CMD" | grep -qE '^git\s+branch(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^git branch/rtk git branch/')
elif echo "$FIRST_CMD" | grep -qE '^git\s+fetch(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^git fetch/rtk git fetch/')
elif echo "$FIRST_CMD" | grep -qE '^git\s+stash(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^git stash/rtk git stash/')
elif echo "$FIRST_CMD" | grep -qE '^git\s+show(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^git show/rtk git show/')

# --- GitHub CLI ---
elif echo "$FIRST_CMD" | grep -qE '^gh\s+(pr|issue|run)(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^gh /rtk gh /')

# --- Cargo ---
elif echo "$FIRST_CMD" | grep -qE '^cargo\s+test(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^cargo test/rtk cargo test/')
elif echo "$FIRST_CMD" | grep -qE '^cargo\s+build(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^cargo build/rtk cargo build/')
elif echo "$FIRST_CMD" | grep -qE '^cargo\s+clippy(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^cargo clippy/rtk cargo clippy/')
elif echo "$FIRST_CMD" | grep -qE '^cargo\s+check(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^cargo check/rtk cargo check/')
elif echo "$FIRST_CMD" | grep -qE '^cargo\s+install(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^cargo install/rtk cargo install/')
elif echo "$FIRST_CMD" | grep -qE '^cargo\s+fmt(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^cargo fmt/rtk cargo fmt/')

# --- File operations ---
elif echo "$FIRST_CMD" | grep -qE '^cat\s+'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^cat /rtk read /')
elif echo "$FIRST_CMD" | grep -qE '^(rg|grep)\s+'; then
  REWRITTEN=$(echo "$CMD" | sed -E 's/^(rg|grep) /rtk grep /')
elif echo "$FIRST_CMD" | grep -qE '^ls(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^ls/rtk ls/')
elif echo "$FIRST_CMD" | grep -qE '^tree(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^tree/rtk tree/')
elif echo "$FIRST_CMD" | grep -qE '^find\s+'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^find /rtk find /')
elif echo "$FIRST_CMD" | grep -qE '^diff\s+'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^diff /rtk diff /')
elif echo "$FIRST_CMD" | grep -qE '^head\s+'; then
  # Transform: head -N file → rtk read file --max-lines N
  # Also handle: head --lines=N file
  if echo "$FIRST_CMD" | grep -qE '^head\s+-[0-9]+\s+'; then
    LINES=$(echo "$FIRST_CMD" | sed -E 's/^head +-([0-9]+) +.+$/\1/')
    FILE=$(echo "$FIRST_CMD" | sed -E 's/^head +-[0-9]+ +(.+)$/\1/')
    REWRITTEN="rtk read $FILE --max-lines $LINES"
  elif echo "$FIRST_CMD" | grep -qE '^head\s+--lines=[0-9]+\s+'; then
    LINES=$(echo "$FIRST_CMD" | sed -E 's/^head +--lines=([0-9]+) +.+$/\1/')
    FILE=$(echo "$FIRST_CMD" | sed -E 's/^head +--lines=[0-9]+ +(.+)$/\1/')
    REWRITTEN="rtk read $FILE --max-lines $LINES"
  fi

# --- JS/TS tooling ---
elif echo "$FIRST_CMD" | grep -qE '^(pnpm\s+)?vitest(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed -E 's/^(pnpm )?vitest/rtk vitest run/')
elif echo "$FIRST_CMD" | grep -qE '^pnpm\s+test(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^pnpm test/rtk vitest run/')
elif echo "$FIRST_CMD" | grep -qE '^pnpm\s+tsc(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^pnpm tsc/rtk tsc/')
elif echo "$FIRST_CMD" | grep -qE '^(npx\s+)?tsc(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed -E 's/^(npx )?tsc/rtk tsc/')
elif echo "$FIRST_CMD" | grep -qE '^pnpm\s+lint(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^pnpm lint/rtk lint/')
elif echo "$FIRST_CMD" | grep -qE '^(npx\s+)?eslint(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed -E 's/^(npx )?eslint/rtk lint/')
elif echo "$FIRST_CMD" | grep -qE '^(npx\s+)?prettier(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed -E 's/^(npx )?prettier/rtk prettier/')
elif echo "$FIRST_CMD" | grep -qE '^(npx\s+)?playwright(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed -E 's/^(npx )?playwright/rtk playwright/')
elif echo "$FIRST_CMD" | grep -qE '^pnpm\s+playwright(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^pnpm playwright/rtk playwright/')
elif echo "$FIRST_CMD" | grep -qE '^(npx\s+)?prisma(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed -E 's/^(npx )?prisma/rtk prisma/')

# --- Containers ---
elif echo "$FIRST_CMD" | grep -qE '^docker\s+(ps|images|logs)(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^docker /rtk docker /')
elif echo "$FIRST_CMD" | grep -qE '^kubectl\s+(get|logs)(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^kubectl /rtk kubectl /')

# --- Network ---
elif echo "$FIRST_CMD" | grep -qE '^curl\s+'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^curl /rtk curl /')
elif echo "$FIRST_CMD" | grep -qE '^wget\s+'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^wget /rtk wget /')

# --- pnpm package management ---
elif echo "$FIRST_CMD" | grep -qE '^pnpm\s+(list|ls|outdated)(\s|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^pnpm /rtk pnpm /')
fi

# If no rewrite needed, approve as-is
if [ -z "$REWRITTEN" ]; then
  exit 0
fi

# Build the updated tool_input with all original fields preserved, only command changed
ORIGINAL_INPUT=$(echo "$INPUT" | jq -c '.tool_input')
UPDATED_INPUT=$(echo "$ORIGINAL_INPUT" | jq --arg cmd "$REWRITTEN" '.command = $cmd')

# Output the rewrite instruction
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
