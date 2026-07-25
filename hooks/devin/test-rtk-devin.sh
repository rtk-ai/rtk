#!/usr/bin/env bash
# Test suite for the Devin CLI RTK PreToolUse hook.
# Feeds mock JSON through `rtk hook devin` and verifies decisions/rewrites.
#
# Usage: bash hooks/devin/test-rtk-devin.sh
# Or from an installed hook dir: bash ~/.config/devin/hooks/rtk/test-rtk-devin.sh

HOOK="${HOOK:-rtk hook devin}"
PASS=0
FAIL=0
TOTAL=0

# Isolate from the user's global Devin permissions so tests are deterministic.
# Devin still picks up project .devin/config.json* if present, but those should
# be empty/nonexistent in the test context.
TEST_CONFIG_DIR=$(mktemp -d)
trap 'rm -rf "$TEST_CONFIG_DIR"' EXIT
export DEVIN_CONFIG_DIR="$TEST_CONFIG_DIR"

# Colors
GREEN='\033[32m'
RED='\033[31m'
DIM='\033[2m'
RESET='\033[0m'

payload() {
  local cmd="$1"
  jq -n --arg cmd "$cmd" '{"tool_name":"exec","tool_input":{"command":$cmd}}'
}

test_rewrite() {
  local description="$1"
  local input_cmd="$2"
  local expected_cmd="$3"  # empty string = expect no rewrite
  TOTAL=$((TOTAL + 1))

  local output
  output=$(payload "$input_cmd" | $HOOK 2>/dev/null) || true

  if [ -z "$expected_cmd" ]; then
    if [ -z "$output" ]; then
      printf "  ${GREEN}PASS${RESET} %s ${DIM}→ (no rewrite)${RESET}\n" "$description"
      PASS=$((PASS + 1))
    else
      local actual
      actual=$(echo "$output" | jq -r '.hookSpecificOutput.updatedInput.command // empty' 2>/dev/null)
      printf "  ${RED}FAIL${RESET} %s\n" "$description"
      printf "       expected: (no rewrite)\n"
      printf "       actual:   %s\n" "$actual"
      FAIL=$((FAIL + 1))
    fi
  else
    local actual
    actual=$(echo "$output" | jq -r '.hookSpecificOutput.updatedInput.command // empty' 2>/dev/null)
    if [ "$actual" = "$expected_cmd" ]; then
      printf "  ${GREEN}PASS${RESET} %s ${DIM}→ %s${RESET}\n" "$description" "$actual"
      PASS=$((PASS + 1))
    else
      printf "  ${RED}FAIL${RESET} %s\n" "$description"
      printf "       expected: %s\n" "$expected_cmd"
      printf "       actual:   %s\n" "$actual"
      FAIL=$((FAIL + 1))
    fi
  fi
}

test_decision() {
  local description="$1"
  local input_cmd="$2"
  local expected_decision="$3"  # approve / block / none
  TOTAL=$((TOTAL + 1))

  local output
  output=$(payload "$input_cmd" | $HOOK 2>/dev/null) || true

  local actual
  if [ -z "$output" ]; then
    actual="none"
  else
    actual=$(echo "$output" | jq -r '.decision // "none"' 2>/dev/null || echo "none")
  fi

  if [ "$actual" = "$expected_decision" ]; then
    printf "  ${GREEN}PASS${RESET} %s ${DIM}→ decision=%s${RESET}\n" "$description" "$actual"
    PASS=$((PASS + 1))
  else
    printf "  ${RED}FAIL${RESET} %s\n" "$description"
    printf "       expected decision: %s\n" "$expected_decision"
    printf "       actual decision:   %s\n" "$actual"
    printf "       output:            %s\n" "$output"
    FAIL=$((FAIL + 1))
  fi
}

echo "============================================"
echo "  Devin CLI RTK Hook Test Suite"
echo "============================================"
echo ""

# ---- SECTION 1: Existing patterns (regression) ----
echo "--- Existing patterns (regression) ---"
test_rewrite "git status" \
  "git status" \
  "rtk git status"

test_rewrite "git log --oneline -10" \
  "git log --oneline -10" \
  "rtk git log --oneline -10"

test_rewrite "git diff HEAD" \
  "git diff HEAD" \
  "rtk git diff HEAD"

test_rewrite "git show abc123" \
  "git show abc123" \
  "rtk git show abc123"

test_rewrite "git add ." \
  "git add ." \
  "rtk git add ."

test_rewrite "cargo test" \
  "cargo test" \
  "rtk cargo test"

test_rewrite "cargo build" \
  "cargo build" \
  "rtk cargo build"

test_rewrite "cargo clippy --all-targets" \
  "cargo clippy --all-targets" \
  "rtk cargo clippy --all-targets"

test_rewrite "ls -la" \
  "ls -la" \
  "rtk ls -la"

test_rewrite "find . -name '*.rs'" \
  "find . -name '*.rs'" \
  "rtk find . -name '*.rs'"

test_rewrite "grep -rn pattern src/" \
  "grep -rn pattern src/" \
  "rtk grep -rn pattern src/"

test_rewrite "rg pattern src/" \
  "rg pattern src/" \
  "rtk rg pattern src/"

test_rewrite "docker ps" \
  "docker ps" \
  "rtk docker ps"

test_rewrite "docker compose logs web" \
  "docker compose logs web" \
  "rtk docker compose logs web"

test_rewrite "kubectl get pods" \
  "kubectl get pods" \
  "rtk kubectl get pods"

test_rewrite "gh pr list" \
  "gh pr list" \
  "rtk gh pr list"

test_rewrite "npx playwright test" \
  "npx playwright test" \
  "rtk playwright test"

test_rewrite "npx vitest" \
  "npx vitest" \
  "rtk vitest"

test_rewrite "npx jest" \
  "npx jest" \
  "rtk jest"

test_rewrite "npm run test:e2e" \
  "npm run test:e2e" \
  "rtk npm run test:e2e"

echo ""

# ---- SECTION 2: Env var prefix handling ----
echo "--- Env var prefix handling ---"
test_rewrite "env + git status" \
  "GIT_PAGER=cat git status" \
  "GIT_PAGER=cat rtk git status"

test_rewrite "multi env + vitest" \
  "NODE_ENV=test CI=1 npx vitest" \
  "NODE_ENV=test CI=1 rtk vitest"

echo ""

# ---- SECTION 3: RTK meta commands ----
echo "--- RTK meta commands (should be auto-approved) ---"
test_decision "rtk gain" \
  "rtk gain" \
  "approve"

test_decision "rtk --version" \
  "rtk --version" \
  "approve"

test_decision "rtk discover" \
  "rtk discover" \
  "approve"

echo ""

# ---- SECTION 4: Already RTK / wrappers ----
echo "--- Already RTK and wrappers ---"
test_rewrite "rtk git status" \
  "rtk git status" \
  "rtk git status"

test_decision "rtk proxy git status (no allow, deferred)" \
  "rtk proxy git status" \
  "none"

echo ""

# ---- SECTION 5: RTK_DISABLED ----
echo "--- RTK_DISABLED (#escape hatch) ---"
test_rewrite "RTK_DISABLED=1 git status (no rewrite)" \
  "RTK_DISABLED=1 git status" \
  ""

test_rewrite "FOO=1 RTK_DISABLED=1 cargo test (no rewrite)" \
  "FOO=1 RTK_DISABLED=1 cargo test" \
  ""

echo ""

# ---- SECTION 6: Should NOT rewrite ----
echo "--- Should NOT rewrite ---"
test_rewrite "heredoc" \
  "cat <<'EOF'
hello
EOF" \
  ""

test_rewrite "echo (no pattern)" \
  "echo hello world" \
  ""

test_rewrite "cd (no pattern)" \
  "cd /tmp" \
  ""

test_rewrite "mkdir (no pattern)" \
  "mkdir -p foo/bar" \
  ""

test_rewrite "python3 script.py (no pattern)" \
  "python3 script.py" \
  ""

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
