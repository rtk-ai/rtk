#!/usr/bin/env bash
# Test suite for rtk hook codex.
# Feeds mock Codex PreToolUse JSON through `rtk hook codex` and verifies deny/pass-through behavior.
#
# Usage: bash hooks/codex/test-rtk-rewrite.sh

RTK="${RTK:-rtk}"
PASS=0
FAIL=0
TOTAL=0

GREEN='\033[32m'
RED='\033[31m'
DIM='\033[2m'
RESET='\033[0m'

codex_bash_input() {
  local cmd="$1"
  jq -cn --arg cmd "$cmd" '{"tool_name":"Bash","tool_input":{"command":$cmd}}'
}

non_bash_input() {
  jq -cn '{"tool_name":"Edit","tool_input":{"command":"git status"}}'
}

test_deny() {
  local description="$1"
  local input_cmd="$2"
  local expected_rtk="$3"
  TOTAL=$((TOTAL + 1))

  local output
  output=$(codex_bash_input "$input_cmd" | "$RTK" hook codex 2>/dev/null) || true

  local decision reason
  decision=$(echo "$output" | jq -r '.hookSpecificOutput.permissionDecision // empty' 2>/dev/null)
  reason=$(echo "$output" | jq -r '.hookSpecificOutput.permissionDecisionReason // empty' 2>/dev/null)

  if [ "$decision" = "deny" ] && echo "$reason" | grep -qF "$expected_rtk"; then
    printf "  ${GREEN}DENY${RESET} %s ${DIM}→ %s${RESET}\n" "$description" "$expected_rtk"
    PASS=$((PASS + 1))
  else
    printf "  ${RED}FAIL${RESET} %s\n" "$description"
    printf "       expected decision: deny, reason containing: %s\n" "$expected_rtk"
    printf "       actual decision:   %s\n" "$decision"
    printf "       actual reason:     %s\n" "$reason"
    FAIL=$((FAIL + 1))
  fi
}

test_allow() {
  local description="$1"
  local input="$2"
  TOTAL=$((TOTAL + 1))

  local output
  output=$(echo "$input" | "$RTK" hook codex 2>/dev/null) || true

  if [ -z "$output" ]; then
    printf "  ${GREEN}PASS${RESET} %s ${DIM}→ (no output)${RESET}\n" "$description"
    PASS=$((PASS + 1))
  else
    printf "  ${RED}FAIL${RESET} %s\n" "$description"
    printf "       expected: (no output)\n"
    printf "       actual:   %s\n" "$output"
    FAIL=$((FAIL + 1))
  fi
}

echo "============================================"
echo "  RTK Codex Hook Test Suite"
echo "============================================"
echo ""

echo "--- Deny with RTK suggestion ---"

test_deny "git status" \
  "git status" \
  "rtk git status"

test_deny "cargo test" \
  "cargo test" \
  "rtk cargo test"

test_deny "gh pr list" \
  "gh pr list" \
  "rtk gh"

echo ""
echo "--- Pass-through ---"

test_allow "already rtk" \
  "$(codex_bash_input "rtk git status")"

test_allow "heredoc" \
  "$(codex_bash_input "cat <<'EOF'
hello
EOF")"

test_allow "unknown command" \
  "$(codex_bash_input "htop")"

test_allow "non-bash tool" \
  "$(non_bash_input)"

echo ""
echo "--- Output format ---"

TOTAL=$((TOTAL + 1))
raw_output=$(codex_bash_input "git status" | "$RTK" hook codex 2>/dev/null)
if echo "$raw_output" | jq . >/dev/null 2>&1; then
  printf "  ${GREEN}PASS${RESET} Codex: output is valid JSON\n"
  PASS=$((PASS + 1))
else
  printf "  ${RED}FAIL${RESET} Codex: output is not valid JSON: %s\n" "$raw_output"
  FAIL=$((FAIL + 1))
fi

TOTAL=$((TOTAL + 1))
decision=$(echo "$raw_output" | jq -r '.hookSpecificOutput.permissionDecision')
if [ "$decision" = "deny" ]; then
  printf "  ${GREEN}PASS${RESET} Codex: hookSpecificOutput.permissionDecision == \"deny\"\n"
  PASS=$((PASS + 1))
else
  printf "  ${RED}FAIL${RESET} Codex: expected \"deny\", got \"%s\"\n" "$decision"
  FAIL=$((FAIL + 1))
fi

TOTAL=$((TOTAL + 1))
reason=$(echo "$raw_output" | jq -r '.hookSpecificOutput.permissionDecisionReason')
if echo "$reason" | grep -qE '`rtk [^`]+`'; then
  printf "  ${GREEN}PASS${RESET} Codex: reason contains backtick-quoted rtk command ${DIM}→ %s${RESET}\n" "$reason"
  PASS=$((PASS + 1))
else
  printf "  ${RED}FAIL${RESET} Codex: reason missing backtick-quoted command: %s\n" "$reason"
  FAIL=$((FAIL + 1))
fi

echo ""
echo "============================================"
if [ $FAIL -eq 0 ]; then
  printf "  ${GREEN}ALL $TOTAL TESTS PASSED${RESET}\n"
else
  printf "  ${RED}$FAIL FAILED${RESET} / $TOTAL total ($PASS passed)\n"
fi
echo "============================================"

exit $FAIL
