#!/bin/bash
# RTK auto-rewrite hook for Claude Code PreToolUse:Bash
# Transparently rewrites raw commands to their rtk equivalents.
# Outputs JSON with updatedInput to modify the command before execution.

# Guards: skip silently if dependencies missing
if ! command -v rtk &>/dev/null || ! command -v jq &>/dev/null; then
  exit 0
fi

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
if echo "$FIRST_CMD" | grep -qE '^git[[:space:]]+status([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^git status/rtk git status/')
elif echo "$FIRST_CMD" | grep -qE '^git[[:space:]]+diff([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^git diff/rtk git diff/')
elif echo "$FIRST_CMD" | grep -qE '^git[[:space:]]+log([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^git log/rtk git log/')
elif echo "$FIRST_CMD" | grep -qE '^git[[:space:]]+add([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^git add/rtk git add/')
elif echo "$FIRST_CMD" | grep -qE '^git[[:space:]]+commit([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^git commit/rtk git commit/')
elif echo "$FIRST_CMD" | grep -qE '^git[[:space:]]+push([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^git push/rtk git push/')
elif echo "$FIRST_CMD" | grep -qE '^git[[:space:]]+pull([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^git pull/rtk git pull/')
elif echo "$FIRST_CMD" | grep -qE '^git[[:space:]]+branch([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^git branch/rtk git branch/')
elif echo "$FIRST_CMD" | grep -qE '^git[[:space:]]+fetch([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^git fetch/rtk git fetch/')
elif echo "$FIRST_CMD" | grep -qE '^git[[:space:]]+stash([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^git stash/rtk git stash/')
elif echo "$FIRST_CMD" | grep -qE '^git[[:space:]]+show([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^git show/rtk git show/')

# --- GitHub CLI ---
elif echo "$FIRST_CMD" | grep -qE '^gh[[:space:]]+(pr|issue|run)([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^gh /rtk gh /')

# --- Cargo ---
elif echo "$FIRST_CMD" | grep -qE '^cargo[[:space:]]+test([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^cargo test/rtk cargo test/')
elif echo "$FIRST_CMD" | grep -qE '^cargo[[:space:]]+build([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^cargo build/rtk cargo build/')
elif echo "$FIRST_CMD" | grep -qE '^cargo[[:space:]]+clippy([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^cargo clippy/rtk cargo clippy/')

# --- File operations ---
elif echo "$FIRST_CMD" | grep -qE '^cat[[:space:]]+'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^cat /rtk read /')
elif echo "$FIRST_CMD" | grep -qE '^(rg|grep)[[:space:]]+'; then
  REWRITTEN=$(echo "$CMD" | sed -E 's/^(rg|grep) /rtk grep /')
elif echo "$FIRST_CMD" | grep -qE '^ls([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^ls/rtk ls/')

# --- JS/TS tooling ---
elif echo "$FIRST_CMD" | grep -qE '^(pnpm[[:space:]]+)?vitest([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed -E 's/^(pnpm )?vitest/rtk vitest run/')
elif echo "$FIRST_CMD" | grep -qE '^pnpm[[:space:]]+test([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^pnpm test/rtk vitest run/')
elif echo "$FIRST_CMD" | grep -qE '^pnpm[[:space:]]+tsc([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^pnpm tsc/rtk tsc/')
elif echo "$FIRST_CMD" | grep -qE '^(npx[[:space:]]+)?tsc([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed -E 's/^(npx )?tsc/rtk tsc/')
elif echo "$FIRST_CMD" | grep -qE '^pnpm[[:space:]]+lint([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^pnpm lint/rtk lint/')
elif echo "$FIRST_CMD" | grep -qE '^(npx[[:space:]]+)?eslint([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed -E 's/^(npx )?eslint/rtk lint/')
elif echo "$FIRST_CMD" | grep -qE '^(npx[[:space:]]+)?prettier([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed -E 's/^(npx )?prettier/rtk prettier/')
elif echo "$FIRST_CMD" | grep -qE '^(npx[[:space:]]+)?playwright([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed -E 's/^(npx )?playwright/rtk playwright/')
elif echo "$FIRST_CMD" | grep -qE '^pnpm[[:space:]]+playwright([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^pnpm playwright/rtk playwright/')
elif echo "$FIRST_CMD" | grep -qE '^(npx[[:space:]]+)?prisma([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed -E 's/^(npx )?prisma/rtk prisma/')

# --- Containers ---
elif echo "$FIRST_CMD" | grep -qE '^docker[[:space:]]+(ps|images|logs)([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^docker /rtk docker /')
elif echo "$FIRST_CMD" | grep -qE '^kubectl[[:space:]]+(get|logs)([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^kubectl /rtk kubectl /')

# --- Network ---
elif echo "$FIRST_CMD" | grep -qE '^curl[[:space:]]+'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^curl /rtk curl /')

# --- pnpm package management ---
elif echo "$FIRST_CMD" | grep -qE '^pnpm[[:space:]]+(list|ls|outdated)([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^pnpm /rtk pnpm /')

# --- Python tooling ---
elif echo "$FIRST_CMD" | grep -qE '^pytest([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^pytest/rtk pytest/')
elif echo "$FIRST_CMD" | grep -qE '^python[[:space:]]+-m[[:space:]]+pytest([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^python -m pytest/rtk pytest/')
elif echo "$FIRST_CMD" | grep -qE '^ruff[[:space:]]+(check|format)([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^ruff /rtk ruff /')
elif echo "$FIRST_CMD" | grep -qE '^pip[[:space:]]+(list|outdated|install|show)([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^pip /rtk pip /')
elif echo "$FIRST_CMD" | grep -qE '^uv[[:space:]]+pip[[:space:]]+(list|outdated|install|show)([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^uv pip /rtk pip /')

# --- Go tooling ---
elif echo "$FIRST_CMD" | grep -qE '^go[[:space:]]+test([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^go test/rtk go test/')
elif echo "$FIRST_CMD" | grep -qE '^go[[:space:]]+build([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^go build/rtk go build/')
elif echo "$FIRST_CMD" | grep -qE '^go[[:space:]]+vet([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^go vet/rtk go vet/')
elif echo "$FIRST_CMD" | grep -qE '^golangci-lint([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^golangci-lint/rtk golangci-lint/')
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
