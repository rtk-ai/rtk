#!/bin/bash
# Test suite for dangerous-actions-blocker.sh
# Usage: bash hooks/claude/test-dangerous-actions-blocker.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
HOOK="$SCRIPT_DIR/dangerous-actions-blocker.sh"
PASSED=0
FAILED=0

# Helper: run hook with a command, return stdout
run_hook() {
  local cmd="$1"
  echo "{\"tool_input\":{\"command\":\"$cmd\"}}" | bash "$HOOK" 2>/dev/null || true
}

# Helper: assert output contains expected decision
assert_decision() {
  local description="$1"
  local cmd="$2"
  local expected_decision="$3"
  local output
  output=$(run_hook "$cmd")

  if [ -z "$expected_decision" ]; then
    # Expect no output (allowed)
    if [ -z "$output" ]; then
      PASSED=$((PASSED + 1))
    else
      FAILED=$((FAILED + 1))
      echo "FAIL: $description"
      echo "  cmd:      $cmd"
      echo "  expected: (allow - no output)"
      echo "  got:      $output"
    fi
  else
    if echo "$output" | grep -q "\"decision\":\"$expected_decision\""; then
      PASSED=$((PASSED + 1))
    else
      FAILED=$((FAILED + 1))
      echo "FAIL: $description"
      echo "  cmd:      $cmd"
      echo "  expected: $expected_decision"
      echo "  got:      ${output:-(empty)}"
    fi
  fi
}

echo "=== Dangerous Actions Blocker — Test Suite ==="
echo ""

# --- DESTRUCTIVE FILE OPERATIONS ---
echo "--- File operations ---"
assert_decision "rm -rf / is blocked"              "rm -rf /"                    "block"
assert_decision "rm -rf ~ is blocked"              "rm -rf ~"                    "block"
assert_decision "rm -rf .. is blocked"             "rm -rf .."                   "block"
assert_decision "rm -rf /etc is blocked"           "rm -rf /etc"                 "block"
assert_decision "rm -rf random dir asks"           "rm -rf myproject"            "ask"
assert_decision "rm -fr also asks"                 "rm -fr myproject"            "ask"
assert_decision "rm -rf node_modules is allowed"   "rm -rf node_modules"         ""
assert_decision "rm -rf dist is allowed"           "rm -rf dist"                 ""
assert_decision "rm -rf .next is allowed"          "rm -rf .next"                ""
assert_decision "rm -rf __pycache__ is allowed"    "rm -rf __pycache__"          ""
assert_decision "rm single file is allowed"        "rm myfile.txt"               ""
assert_decision "docker exec rm is allowed"        "docker exec mycontainer rm -rf /app/tmp" ""
assert_decision "kubectl exec rm is allowed"       "kubectl exec mypod -- rm -rf /app/cache" ""

# --- GIT OPERATIONS ---
echo "--- Git operations ---"
assert_decision "git push --force is blocked"      "git push --force"            "block"
assert_decision "git push -f is blocked"           "git push origin main -f"     "block"
assert_decision "git push --force-with-lease ok"   "git push --force-with-lease" ""
assert_decision "git reset --hard asks"            "git reset --hard HEAD~1"     "ask"
assert_decision "git clean -f asks"                "git clean -f"                "ask"
assert_decision "git clean -fd asks"               "git clean -fd"               "ask"
assert_decision "git checkout -- . asks"           "git checkout -- ."           "ask"
assert_decision "git branch -D asks"               "git branch -D my-branch"    "ask"
assert_decision "normal git push is allowed"       "git push origin main"        ""
assert_decision "git status is allowed"            "git status"                  ""
assert_decision "git commit is allowed"            "git commit -m fix"           ""

# --- SECRETS ---
echo "--- Secrets exposure ---"
assert_decision "cat .env is blocked"              "cat .env"                    "block"
assert_decision "cat .pem is blocked"              "cat server.pem"              "block"
assert_decision "cat .key is blocked"              "cat private.key"             "block"
assert_decision "head .credentials is blocked"     "head .credentials"           "block"
assert_decision "API key in command is blocked"    "ANTHROPIC_API_KEY=sk-123 curl" "block"
assert_decision "cat normal file is allowed"       "cat README.md"               ""

# --- DATABASE ---
echo "--- Database operations ---"
assert_decision "DROP TABLE is blocked"            "psql -c DROP TABLE users;"   "block"
assert_decision "TRUNCATE TABLE is blocked"        "mysql -e TRUNCATE TABLE logs;" "block"
assert_decision "SELECT is allowed"                "psql -c SELECT * FROM users;" ""

# --- DOCKER ---
echo "--- Docker operations ---"
assert_decision "docker system prune -a asks"      "docker system prune -a"      "ask"
assert_decision "docker system prune --all asks"   "docker system prune --all"   "ask"
assert_decision "docker ps is allowed"             "docker ps"                   ""

# --- EDGE CASES ---
echo "--- Edge cases ---"
assert_decision "empty input is allowed"           ""                            ""
assert_decision "safe command is allowed"           "ls -la"                     ""
assert_decision "cargo build is allowed"           "cargo build --release"       ""

echo ""
echo "=== Results: $PASSED passed, $FAILED failed ==="

if [ "$FAILED" -gt 0 ]; then
  exit 1
fi
