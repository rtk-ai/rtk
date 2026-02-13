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

# --- Git commands (normalize past global options to find subcommand) ---
GIT_SUBCMD=""
if echo "$FIRST_CMD" | grep -qE '^git[[:space:]]'; then
  GIT_SUBCMD=$(echo "$FIRST_CMD" | sed -E \
    -e 's/^git[[:space:]]+//' \
    -e 's/(-C|-c)[[:space:]]+[^[:space:]]+[[:space:]]*//g' \
    -e 's/--[a-z-]+=[^[:space:]]+[[:space:]]*//g' \
    -e 's/--(no-pager|no-optional-locks|bare|literal-pathspecs)[[:space:]]*//g' \
    -e 's/^[[:space:]]+//')
fi

if [ -n "$GIT_SUBCMD" ]; then
  case "$GIT_SUBCMD" in
    status|status\ *)  REWRITTEN="rtk $CMD" ;;
    diff|diff\ *)      REWRITTEN="rtk $CMD" ;;
    log|log\ *)        REWRITTEN="rtk $CMD" ;;
    add|add\ *)        REWRITTEN="rtk $CMD" ;;
    commit|commit\ *)  REWRITTEN="rtk $CMD" ;;
    push|push\ *)      REWRITTEN="rtk $CMD" ;;
    pull|pull\ *)      REWRITTEN="rtk $CMD" ;;
    branch|branch\ *)  REWRITTEN="rtk $CMD" ;;
    fetch|fetch\ *)    REWRITTEN="rtk $CMD" ;;
    stash|stash\ *)    REWRITTEN="rtk $CMD" ;;
    show|show\ *)      REWRITTEN="rtk $CMD" ;;
  esac

# --- GitHub CLI ---
elif echo "$FIRST_CMD" | grep -qE '^gh[[:space:]]+(pr|issue|run)([[:space:]]|$)'; then
  REWRITTEN=$(echo "$CMD" | sed 's/^gh /rtk gh /')

# --- Cargo (normalize past +toolchain to find subcommand) ---
elif echo "$FIRST_CMD" | grep -qE '^cargo[[:space:]]'; then
  CARGO_SUBCMD=$(echo "$FIRST_CMD" | sed -E 's/^cargo[[:space:]]+(\+[^[:space:]]+[[:space:]]+)?//')
  case "$CARGO_SUBCMD" in
    test|test\ *)     REWRITTEN="rtk $CMD" ;;
    build|build\ *)   REWRITTEN="rtk $CMD" ;;
    clippy|clippy\ *) REWRITTEN="rtk $CMD" ;;
  esac

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

# --- Containers (normalize past global options to find subcommand) ---
elif echo "$FIRST_CMD" | grep -qE '^docker[[:space:]]'; then
  DOCKER_SUBCMD=$(echo "$FIRST_CMD" | sed -E \
    -e 's/^docker[[:space:]]+//' \
    -e 's/(-H|--context|--config)[[:space:]]+[^[:space:]]+[[:space:]]*//g' \
    -e 's/--[a-z-]+=[^[:space:]]+[[:space:]]*//g' \
    -e 's/^[[:space:]]+//')
  case "$DOCKER_SUBCMD" in
    ps|ps\ *|images|images\ *|logs|logs\ *) REWRITTEN="rtk $CMD" ;;
  esac
elif echo "$FIRST_CMD" | grep -qE '^kubectl[[:space:]]'; then
  KUBE_SUBCMD=$(echo "$FIRST_CMD" | sed -E \
    -e 's/^kubectl[[:space:]]+//' \
    -e 's/(--context|--kubeconfig|--namespace|-n)[[:space:]]+[^[:space:]]+[[:space:]]*//g' \
    -e 's/--[a-z-]+=[^[:space:]]+[[:space:]]*//g' \
    -e 's/^[[:space:]]+//')
  case "$KUBE_SUBCMD" in
    get|get\ *|logs|logs\ *) REWRITTEN="rtk $CMD" ;;
  esac

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
