#!/bin/bash
# RTK auto-rewrite hook for Claude Code PreToolUse:Bash
# Transparently rewrites raw commands to their rtk equivalents.
# Outputs JSON with updatedInput to modify the command before execution.

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

# Extract the first meaningful command (before pipes, &&, etc.)
# We only rewrite if the FIRST command in a chain matches.
FIRST_CMD="$CMD"

# Skip if already using rtk
case "$FIRST_CMD" in
  rtk\ *|*/rtk\ *) _rtk_audit_log "skip:already_rtk" "$CMD"; exit 0 ;;
esac

# Skip commands with heredocs, variable assignments as the whole command, etc.
case "$FIRST_CMD" in
  *'<<'*) _rtk_audit_log "skip:heredoc" "$CMD"; exit 0 ;;
esac

# Strip leading env var assignments for pattern matching
# e.g., "TEST_SESSION_ID=2 npx playwright test" → match against "npx playwright test"
# but preserve them in the rewritten command for execution.
ENV_PREFIX=$(echo "$FIRST_CMD" | grep -oE '^([A-Za-z_][A-Za-z0-9_]*=[^ ]* +)+' || echo "")
if [ -n "$ENV_PREFIX" ]; then
  MATCH_CMD="${FIRST_CMD:${#ENV_PREFIX}}"
  CMD_BODY="${CMD:${#ENV_PREFIX}}"
else
  MATCH_CMD="$FIRST_CMD"
  CMD_BODY="$CMD"
fi

REWRITTEN=""

# --- Extract git global flags (-C <path>) before subcommand matching ---
GIT_GLOBAL_FLAGS=""
GIT_SUBCMD="$MATCH_CMD"
GIT_SUBCMD_BODY="$CMD_BODY"
if echo "$MATCH_CMD" | grep -qE '^git[[:space:]]+-C[[:space:]]'; then
  # Extract all -C <path> flags (may be repeated)
  GIT_GLOBAL_FLAGS=$(echo "$MATCH_CMD" | sed -E 's/^git[[:space:]]+((-C[[:space:]]+[^[:space:]]+[[:space:]]+)+).*/\1/')
  # Rebuild MATCH_CMD/CMD_BODY without the global flags for subcommand matching
  GIT_SUBCMD="git $(echo "$MATCH_CMD" | sed -E 's/^git[[:space:]]+(-C[[:space:]]+[^[:space:]]+[[:space:]]+)+//')"
  GIT_SUBCMD_BODY="git $(echo "$CMD_BODY" | sed -E 's/^git[[:space:]]+(-C[[:space:]]+[^[:space:]]+[[:space:]]+)+//')"
fi

# Helper: rewrite git command preserving -C flags
# Usage: _rtk_git_rewrite <subcmd>
# Reads GIT_SUBCMD_BODY, GIT_GLOBAL_FLAGS, ENV_PREFIX from outer scope
_rtk_git_rewrite() {
  local subcmd="$1"
  local rest="${GIT_SUBCMD_BODY#git ${subcmd}}"
  if [ -n "$GIT_GLOBAL_FLAGS" ]; then
    REWRITTEN="${ENV_PREFIX}rtk git ${GIT_GLOBAL_FLAGS}${subcmd}${rest}"
  else
    REWRITTEN="${ENV_PREFIX}rtk git ${subcmd}${rest}"
  fi
}

# --- Git commands ---
if echo "$GIT_SUBCMD" | grep -qE '^git[[:space:]]+status([[:space:]]|$)'; then
  _rtk_git_rewrite "status"
elif echo "$GIT_SUBCMD" | grep -qE '^git[[:space:]]+diff([[:space:]]|$)'; then
  _rtk_git_rewrite "diff"
elif echo "$GIT_SUBCMD" | grep -qE '^git[[:space:]]+log([[:space:]]|$)'; then
  _rtk_git_rewrite "log"
elif echo "$GIT_SUBCMD" | grep -qE '^git[[:space:]]+add([[:space:]]|$)'; then
  _rtk_git_rewrite "add"
elif echo "$GIT_SUBCMD" | grep -qE '^git[[:space:]]+commit([[:space:]]|$)'; then
  _rtk_git_rewrite "commit"
elif echo "$GIT_SUBCMD" | grep -qE '^git[[:space:]]+push([[:space:]]|$)'; then
  _rtk_git_rewrite "push"
elif echo "$GIT_SUBCMD" | grep -qE '^git[[:space:]]+pull([[:space:]]|$)'; then
  _rtk_git_rewrite "pull"
elif echo "$GIT_SUBCMD" | grep -qE '^git[[:space:]]+branch([[:space:]]|$)'; then
  _rtk_git_rewrite "branch"
elif echo "$GIT_SUBCMD" | grep -qE '^git[[:space:]]+fetch([[:space:]]|$)'; then
  _rtk_git_rewrite "fetch"
elif echo "$GIT_SUBCMD" | grep -qE '^git[[:space:]]+stash([[:space:]]|$)'; then
  _rtk_git_rewrite "stash"
elif echo "$GIT_SUBCMD" | grep -qE '^git[[:space:]]+show([[:space:]]|$)'; then
  _rtk_git_rewrite "show"

# --- GitHub CLI (added: api, release) ---
elif echo "$MATCH_CMD" | grep -qE '^gh[[:space:]]+(pr|issue|run|api|release)([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^gh /rtk gh /')"

# --- Cargo ---
elif echo "$MATCH_CMD" | grep -qE '^cargo[[:space:]]+test([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^cargo test/rtk cargo test/')"
elif echo "$MATCH_CMD" | grep -qE '^cargo[[:space:]]+build([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^cargo build/rtk cargo build/')"
elif echo "$MATCH_CMD" | grep -qE '^cargo[[:space:]]+clippy([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^cargo clippy/rtk cargo clippy/')"
elif echo "$MATCH_CMD" | grep -qE '^cargo[[:space:]]+check([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^cargo check/rtk cargo check/')"
elif echo "$MATCH_CMD" | grep -qE '^cargo[[:space:]]+install([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^cargo install/rtk cargo install/')"
elif echo "$MATCH_CMD" | grep -qE '^cargo[[:space:]]+nextest([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^cargo nextest/rtk cargo nextest/')"
elif echo "$MATCH_CMD" | grep -qE '^cargo[[:space:]]+fmt([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^cargo fmt/rtk cargo fmt/')"

# --- File operations ---
elif echo "$MATCH_CMD" | grep -qE '^cat[[:space:]]+'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^cat /rtk read /')"
elif echo "$MATCH_CMD" | grep -qE '^(rg|grep)[[:space:]]+'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed -E 's/^(rg|grep) /rtk grep /')"
elif echo "$MATCH_CMD" | grep -qE '^ls([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^ls/rtk ls/')"
elif echo "$MATCH_CMD" | grep -qE '^tree([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^tree/rtk tree/')"
elif echo "$MATCH_CMD" | grep -qE '^find[[:space:]]+'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^find /rtk find /')"
elif echo "$MATCH_CMD" | grep -qE '^diff[[:space:]]+'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^diff /rtk diff /')"
elif echo "$MATCH_CMD" | grep -qE '^head[[:space:]]+'; then
  # Transform: head -N file → rtk read file --max-lines N
  # Also handle: head --lines=N file
  if echo "$MATCH_CMD" | grep -qE '^head[[:space:]]+-[0-9]+[[:space:]]+'; then
    LINES=$(echo "$MATCH_CMD" | sed -E 's/^head +-([0-9]+) +.+$/\1/')
    FILE=$(echo "$MATCH_CMD" | sed -E 's/^head +-[0-9]+ +(.+)$/\1/')
    REWRITTEN="${ENV_PREFIX}rtk read $FILE --max-lines $LINES"
  elif echo "$MATCH_CMD" | grep -qE '^head[[:space:]]+--lines=[0-9]+[[:space:]]+'; then
    LINES=$(echo "$MATCH_CMD" | sed -E 's/^head +--lines=([0-9]+) +.+$/\1/')
    FILE=$(echo "$MATCH_CMD" | sed -E 's/^head +--lines=[0-9]+ +(.+)$/\1/')
    REWRITTEN="${ENV_PREFIX}rtk read $FILE --max-lines $LINES"
  fi

# --- JS/TS tooling (added: npm run, npm test, vue-tsc) ---
elif echo "$MATCH_CMD" | grep -qE '^(pnpm[[:space:]]+)?(npx[[:space:]]+)?vitest([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed -E 's/^(pnpm )?(npx )?vitest( run)?/rtk vitest run/')"
elif echo "$MATCH_CMD" | grep -qE '^pnpm[[:space:]]+test([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^pnpm test/rtk vitest run/')"
elif echo "$MATCH_CMD" | grep -qE '^npm[[:space:]]+test([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^npm test/rtk npm test/')"
elif echo "$MATCH_CMD" | grep -qE '^npm[[:space:]]+run[[:space:]]+'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^npm run /rtk npm /')"
elif echo "$MATCH_CMD" | grep -qE '^(npx[[:space:]]+)?vue-tsc([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed -E 's/^(npx )?vue-tsc/rtk tsc/')"
elif echo "$MATCH_CMD" | grep -qE '^pnpm[[:space:]]+tsc([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^pnpm tsc/rtk tsc/')"
elif echo "$MATCH_CMD" | grep -qE '^(npx[[:space:]]+)?tsc([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed -E 's/^(npx )?tsc/rtk tsc/')"
elif echo "$MATCH_CMD" | grep -qE '^pnpm[[:space:]]+lint([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^pnpm lint/rtk lint/')"
elif echo "$MATCH_CMD" | grep -qE '^(npx[[:space:]]+)?eslint([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed -E 's/^(npx )?eslint/rtk lint/')"
elif echo "$MATCH_CMD" | grep -qE '^(npx[[:space:]]+)?prettier([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed -E 's/^(npx )?prettier/rtk prettier/')"
elif echo "$MATCH_CMD" | grep -qE '^(npx[[:space:]]+)?playwright([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed -E 's/^(npx )?playwright/rtk playwright/')"
elif echo "$MATCH_CMD" | grep -qE '^pnpm[[:space:]]+playwright([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^pnpm playwright/rtk playwright/')"
elif echo "$MATCH_CMD" | grep -qE '^(npx[[:space:]]+)?prisma([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed -E 's/^(npx )?prisma/rtk prisma/')"

# --- Containers (added: docker compose, docker run/build/exec, kubectl describe/apply) ---
elif echo "$MATCH_CMD" | grep -qE '^docker[[:space:]]+compose([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^docker /rtk docker /')"
elif echo "$MATCH_CMD" | grep -qE '^docker[[:space:]]+(ps|images|logs|run|build|exec)([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^docker /rtk docker /')"
elif echo "$MATCH_CMD" | grep -qE '^kubectl[[:space:]]+(get|logs|describe|apply)([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^kubectl /rtk kubectl /')"

# --- Network ---
elif echo "$MATCH_CMD" | grep -qE '^curl[[:space:]]+'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^curl /rtk curl /')"
elif echo "$MATCH_CMD" | grep -qE '^wget[[:space:]]+'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^wget /rtk wget /')"

# --- pnpm package management ---
elif echo "$MATCH_CMD" | grep -qE '^pnpm[[:space:]]+(list|ls|outdated)([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^pnpm /rtk pnpm /')"

# --- Python tooling ---
elif echo "$MATCH_CMD" | grep -qE '^pytest([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^pytest/rtk pytest/')"
elif echo "$MATCH_CMD" | grep -qE '^python[[:space:]]+-m[[:space:]]+pytest([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^python -m pytest/rtk pytest/')"
elif echo "$MATCH_CMD" | grep -qE '^ruff[[:space:]]+(check|format)([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^ruff /rtk ruff /')"
elif echo "$MATCH_CMD" | grep -qE '^pip[[:space:]]+(list|outdated|install|show)([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^pip /rtk pip /')"
elif echo "$MATCH_CMD" | grep -qE '^uv[[:space:]]+pip[[:space:]]+(list|outdated|install|show)([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^uv pip /rtk pip /')"

# --- Go tooling ---
elif echo "$MATCH_CMD" | grep -qE '^go[[:space:]]+test([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^go test/rtk go test/')"
elif echo "$MATCH_CMD" | grep -qE '^go[[:space:]]+build([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^go build/rtk go build/')"
elif echo "$MATCH_CMD" | grep -qE '^go[[:space:]]+vet([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^go vet/rtk go vet/')"
elif echo "$MATCH_CMD" | grep -qE '^golangci-lint([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^golangci-lint/rtk golangci-lint/')"
fi

# If no rewrite needed, approve as-is
if [ -z "$REWRITTEN" ]; then
  _rtk_audit_log "skip:no_match" "$CMD"
  exit 0
fi

_rtk_audit_log "rewrite" "$CMD" "$REWRITTEN"

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
