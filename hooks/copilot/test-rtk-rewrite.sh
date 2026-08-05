#!/usr/bin/env bash
# Test suite for rtk hook (cross-platform preToolUse handler).
# Feeds mock preToolUse JSON through `rtk hook` and verifies allow/deny decisions.
#
# Usage: bash hooks/test-copilot-rtk-rewrite.sh
#
# Copilot CLI input format:
#   {"toolName":"bash","toolArgs":"{\"command\":\"...\"}"}
#   Output on intercept: {"permissionDecision":"deny","permissionDecisionReason":"..."}
#
# VS Code Copilot Chat input format:
#   {"tool_name":"Bash","tool_input":{"command":"..."}}
#   Output on intercept: {"hookSpecificOutput":{"permissionDecision":"allow","updatedInput":{...}}}
#
# Output on pass-through: empty (exit 0)

RTK="${RTK:-rtk}"
PASS=0
FAIL=0
TOTAL=0

# Colors
GREEN='\033[32m'
RED='\033[31m'
DIM='\033[2m'
RESET='\033[0m'

# Build a Copilot CLI preToolUse input JSON
copilot_bash_input() {
  local cmd="$1"
  local tool_args
  tool_args=$(jq -cn --arg cmd "$cmd" '{"command":$cmd}')
  jq -cn --arg ta "$tool_args" '{"toolName":"bash","toolArgs":$ta}'
}

# Build a VS Code Copilot Chat preToolUse input JSON
vscode_bash_input() {
  local cmd="$1"
  jq -cn --arg cmd "$cmd" '{"tool_name":"Bash","tool_input":{"command":$cmd}}'
}

# Build a non-bash tool input
tool_input() {
  local tool_name="$1"
  jq -cn --arg t "$tool_name" '{"toolName":$t,"toolArgs":"{}"}'
}

# Assert Copilot CLI: hook denies and reason contains the expected rtk command
test_deny() {
  local description="$1"
  local input_cmd="$2"
  local expected_rtk="$3"
  TOTAL=$((TOTAL + 1))

  local output
  output=$(copilot_bash_input "$input_cmd" | "$RTK" hook 2>/dev/null) || true

  local decision reason
  decision=$(echo "$output" | jq -r '.permissionDecision // empty' 2>/dev/null)
  reason=$(echo "$output" | jq -r '.permissionDecisionReason // empty' 2>/dev/null)

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

# Assert VS Code Copilot Chat: hook returns updatedInput (allow) with rewritten command
test_vscode_rewrite() {
  local description="$1"
  local input_cmd="$2"
  local expected_rtk="$3"
  TOTAL=$((TOTAL + 1))

  local output
  output=$(vscode_bash_input "$input_cmd" | "$RTK" hook 2>/dev/null) || true

  local decision updated_cmd
  decision=$(echo "$output" | jq -r '.hookSpecificOutput.permissionDecision // empty' 2>/dev/null)
  updated_cmd=$(echo "$output" | jq -r '.hookSpecificOutput.updatedInput.command // empty' 2>/dev/null)

  if [ "$decision" = "allow" ] && echo "$updated_cmd" | grep -qF "$expected_rtk"; then
    printf "  ${GREEN}REWRITE${RESET} %s ${DIM}→ %s${RESET}\n" "$description" "$updated_cmd"
    PASS=$((PASS + 1))
  else
    printf "  ${RED}FAIL${RESET} %s\n" "$description"
    printf "       expected decision: allow, updatedInput containing: %s\n" "$expected_rtk"
    printf "       actual decision:   %s\n" "$decision"
    printf "       actual updatedInput: %s\n" "$updated_cmd"
    FAIL=$((FAIL + 1))
  fi
}

# Assert the hook emits no output (pass-through)
test_allow() {
  local description="$1"
  local input="$2"
  TOTAL=$((TOTAL + 1))

  local output
  output=$(echo "$input" | "$RTK" hook 2>/dev/null) || true

  if [ -z "$output" ]; then
    printf "  ${GREEN}PASS${RESET} %s ${DIM}→ (allow)${RESET}\n" "$description"
    PASS=$((PASS + 1))
  else
    local decision
    decision=$(echo "$output" | jq -r '.permissionDecision // .hookSpecificOutput.permissionDecision // empty' 2>/dev/null)
    printf "  ${RED}FAIL${RESET} %s\n" "$description"
    printf "       expected: (no output)\n"
    printf "       actual:   permissionDecision=%s\n" "$decision"
    FAIL=$((FAIL + 1))
  fi
}

echo "============================================"
echo "  RTK Hook Test Suite (rtk hook)"
echo "============================================"
echo ""

# ---- SECTION 1: Copilot CLI — commands that should be denied ----
echo "--- Copilot CLI: intercepted (deny with rtk suggestion) ---"

test_deny "git status" \
  "git status" \
  "rtk git status"

test_deny "git log --oneline -10" \
  "git log --oneline -10" \
  "rtk git log"

test_deny "git diff HEAD" \
  "git diff HEAD" \
  "rtk git diff"

test_deny "cargo test" \
  "cargo test" \
  "rtk cargo test"

test_deny "cargo clippy --all-targets" \
  "cargo clippy --all-targets" \
  "rtk cargo clippy"

test_deny "cargo build" \
  "cargo build" \
  "rtk cargo build"

test_deny "grep -rn pattern src/" \
  "grep -rn pattern src/" \
  "rtk grep"

test_deny "gh pr list" \
  "gh pr list" \
  "rtk gh"

echo ""

# ---- SECTION 2: VS Code Copilot Chat — commands that should be rewritten via updatedInput ----
echo "--- VS Code Copilot Chat: intercepted (updatedInput rewrite) ---"

test_vscode_rewrite "git status" \
  "git status" \
  "rtk git status"

test_vscode_rewrite "cargo test" \
  "cargo test" \
  "rtk cargo test"

test_vscode_rewrite "gh pr list" \
  "gh pr list" \
  "rtk gh"

echo ""

# ---- SECTION 3: Pass-through cases ----
echo "--- Pass-through (allow silently) ---"

test_allow "Copilot CLI: already rtk: rtk git status" \
  "$(copilot_bash_input "rtk git status")"

test_allow "Copilot CLI: already rtk: rtk cargo test" \
  "$(copilot_bash_input "rtk cargo test")"

test_allow "Copilot CLI: heredoc" \
  "$(copilot_bash_input "cat <<'EOF'
hello
EOF")"

test_allow "Copilot CLI: unknown command: htop" \
  "$(copilot_bash_input "htop")"

test_allow "Copilot CLI: unknown command: echo" \
  "$(copilot_bash_input "echo hello world")"

test_allow "Copilot CLI: non-bash tool: view" \
  "$(tool_input "view")"

test_allow "Copilot CLI: non-bash tool: edit" \
  "$(tool_input "edit")"

test_allow "VS Code: already rtk" \
  "$(vscode_bash_input "rtk git status")"

test_allow "VS Code: non-bash tool: editFiles" \
  "$(jq -cn '{"tool_name":"editFiles"}')"

echo ""

# ---- SECTION 4: Output format assertions ----
echo "--- Output format ---"

# Copilot CLI output format
TOTAL=$((TOTAL + 1))
raw_output=$(copilot_bash_input "git status" | "$RTK" hook 2>/dev/null)

if echo "$raw_output" | jq . >/dev/null 2>&1; then
  printf "  ${GREEN}PASS${RESET} Copilot CLI: output is valid JSON\n"
  PASS=$((PASS + 1))
else
  printf "  ${RED}FAIL${RESET} Copilot CLI: output is not valid JSON: %s\n" "$raw_output"
  FAIL=$((FAIL + 1))
fi

TOTAL=$((TOTAL + 1))
decision=$(echo "$raw_output" | jq -r '.permissionDecision')
if [ "$decision" = "deny" ]; then
  printf "  ${GREEN}PASS${RESET} Copilot CLI: permissionDecision == \"deny\"\n"
  PASS=$((PASS + 1))
else
  printf "  ${RED}FAIL${RESET} Copilot CLI: expected \"deny\", got \"%s\"\n" "$decision"
  FAIL=$((FAIL + 1))
fi

TOTAL=$((TOTAL + 1))
reason=$(echo "$raw_output" | jq -r '.permissionDecisionReason')
if echo "$reason" | grep -qE '`rtk [^`]+`'; then
  printf "  ${GREEN}PASS${RESET} Copilot CLI: reason contains backtick-quoted rtk command ${DIM}→ %s${RESET}\n" "$reason"
  PASS=$((PASS + 1))
else
  printf "  ${RED}FAIL${RESET} Copilot CLI: reason missing backtick-quoted command: %s\n" "$reason"
  FAIL=$((FAIL + 1))
fi

# VS Code output format
TOTAL=$((TOTAL + 1))
vscode_output=$(vscode_bash_input "git status" | "$RTK" hook 2>/dev/null)

if echo "$vscode_output" | jq . >/dev/null 2>&1; then
  printf "  ${GREEN}PASS${RESET} VS Code: output is valid JSON\n"
  PASS=$((PASS + 1))
else
  printf "  ${RED}FAIL${RESET} VS Code: output is not valid JSON: %s\n" "$vscode_output"
  FAIL=$((FAIL + 1))
fi

TOTAL=$((TOTAL + 1))
vscode_decision=$(echo "$vscode_output" | jq -r '.hookSpecificOutput.permissionDecision')
if [ "$vscode_decision" = "allow" ]; then
  printf "  ${GREEN}PASS${RESET} VS Code: hookSpecificOutput.permissionDecision == \"allow\"\n"
  PASS=$((PASS + 1))
else
  printf "  ${RED}FAIL${RESET} VS Code: expected \"allow\", got \"%s\"\n" "$vscode_decision"
  FAIL=$((FAIL + 1))
fi

TOTAL=$((TOTAL + 1))
vscode_updated=$(echo "$vscode_output" | jq -r '.hookSpecificOutput.updatedInput.command')
if echo "$vscode_updated" | grep -q "^rtk "; then
  printf "  ${GREEN}PASS${RESET} VS Code: updatedInput.command starts with rtk ${DIM}→ %s${RESET}\n" "$vscode_updated"
  PASS=$((PASS + 1))
else
  printf "  ${RED}FAIL${RESET} VS Code: updatedInput.command should start with rtk: %s\n" "$vscode_updated"
  FAIL=$((FAIL + 1))
fi

echo ""

# ---- SUMMARY ----
echo "============================================"
if [ $FAIL -eq 0 ]; then
  printf "  ${GREEN}ALL $TOTAL TESTS PASSED${RESET}\n"
else
  printf "  ${RED}$FAIL FAILED${RESET} / $TOTAL total ($PASS passed)\n"
fi
echo "============================================"

exit $FAIL
