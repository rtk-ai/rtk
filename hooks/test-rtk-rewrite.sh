#!/bin/bash
# Test suite for rtk-rewrite.sh
# Feeds mock JSON through the hook and verifies the rewritten commands.
#
# Usage: bash ~/.claude/hooks/test-rtk-rewrite.sh

HOOK="$HOME/.claude/hooks/rtk-rewrite.sh"
PASS=0
FAIL=0
TOTAL=0

# Colors
GREEN='\033[32m'
RED='\033[31m'
DIM='\033[2m'
RESET='\033[0m'

test_rewrite() {
  local description="$1"
  local input_cmd="$2"
  local expected_cmd="$3"  # empty string = expect no rewrite
  TOTAL=$((TOTAL + 1))

  local input_json
  input_json=$(jq -n --arg cmd "$input_cmd" '{"tool_name":"Bash","tool_input":{"command":$cmd}}')
  local output
  output=$(echo "$input_json" | bash "$HOOK" 2>/dev/null) || true

  if [ -z "$expected_cmd" ]; then
    # Expect no rewrite (hook exits 0 with no output)
    if [ -z "$output" ]; then
      printf "  ${GREEN}PASS${RESET} %s ${DIM}→ (no rewrite)${RESET}\n" "$description"
      PASS=$((PASS + 1))
    else
      local actual
      actual=$(echo "$output" | jq -r '.hookSpecificOutput.updatedInput.command // empty')
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

echo "============================================"
echo "  RTK Rewrite Hook Test Suite"
echo "============================================"
echo ""

# ---- SECTION 1: Existing patterns (regression tests) ----
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

test_rewrite "gh pr list" \
  "gh pr list" \
  "rtk gh pr list"

test_rewrite "npx playwright test" \
  "npx playwright test" \
  "rtk playwright test"

test_rewrite "ls -la" \
  "ls -la" \
  "rtk ls -la"

test_rewrite "curl -s https://example.com" \
  "curl -s https://example.com" \
  "rtk curl -s https://example.com"

test_rewrite "cat package.json" \
  "cat package.json" \
  "rtk read package.json"

test_rewrite "grep -rn pattern src/" \
  "grep -rn pattern src/" \
  "rtk grep -rn pattern src/"

test_rewrite "rg pattern src/" \
  "rg pattern src/" \
  "rtk grep pattern src/"

test_rewrite "cargo test" \
  "cargo test" \
  "rtk cargo test"

test_rewrite "npx prisma migrate" \
  "npx prisma migrate" \
  "rtk prisma migrate"

echo ""

# ---- SECTION 2: Env var prefix handling (THE BIG FIX) ----
echo "--- Env var prefix handling (new) ---"
test_rewrite "env + playwright" \
  "TEST_SESSION_ID=2 npx playwright test --config=foo" \
  "TEST_SESSION_ID=2 rtk playwright test --config=foo"

test_rewrite "env + git status" \
  "GIT_PAGER=cat git status" \
  "GIT_PAGER=cat rtk git status"

test_rewrite "env + git log" \
  "GIT_PAGER=cat git log --oneline -10" \
  "GIT_PAGER=cat rtk git log --oneline -10"

test_rewrite "multi env + vitest" \
  "NODE_ENV=test CI=1 npx vitest run" \
  "NODE_ENV=test CI=1 rtk vitest run"

test_rewrite "env + ls" \
  "LANG=C ls -la" \
  "LANG=C rtk ls -la"

test_rewrite "env + npm run" \
  "NODE_ENV=test npm run test:e2e" \
  "NODE_ENV=test rtk npm test:e2e"

test_rewrite "env + docker compose" \
  "COMPOSE_PROJECT_NAME=test docker compose up -d" \
  "COMPOSE_PROJECT_NAME=test rtk docker compose up -d"

echo ""

# ---- SECTION 3: New patterns ----
echo "--- New patterns ---"
test_rewrite "npm run test:e2e" \
  "npm run test:e2e" \
  "rtk npm test:e2e"

test_rewrite "npm run build" \
  "npm run build" \
  "rtk npm build"

test_rewrite "npm test" \
  "npm test" \
  "rtk npm test"

test_rewrite "vue-tsc -b" \
  "vue-tsc -b" \
  "rtk tsc -b"

test_rewrite "npx vue-tsc --noEmit" \
  "npx vue-tsc --noEmit" \
  "rtk tsc --noEmit"

test_rewrite "docker compose up -d" \
  "docker compose up -d" \
  "rtk docker compose up -d"

test_rewrite "docker compose logs postgrest" \
  "docker compose logs postgrest" \
  "rtk docker compose logs postgrest"

test_rewrite "docker compose down" \
  "docker compose down" \
  "rtk docker compose down"

test_rewrite "docker run --rm postgres" \
  "docker run --rm postgres" \
  "rtk docker run --rm postgres"

test_rewrite "docker exec -it db psql" \
  "docker exec -it db psql" \
  "rtk docker exec -it db psql"

test_rewrite "find (NOT rewritten — different arg format)" \
  "find . -name '*.ts'" \
  ""

test_rewrite "tree (NOT rewritten — different arg format)" \
  "tree src/" \
  ""

test_rewrite "wget (NOT rewritten — different arg format)" \
  "wget https://example.com/file" \
  ""

test_rewrite "gh api repos/owner/repo" \
  "gh api repos/owner/repo" \
  "rtk gh api repos/owner/repo"

test_rewrite "gh release list" \
  "gh release list" \
  "rtk gh release list"

test_rewrite "kubectl describe pod foo" \
  "kubectl describe pod foo" \
  "rtk kubectl describe pod foo"

test_rewrite "kubectl apply -f deploy.yaml" \
  "kubectl apply -f deploy.yaml" \
  "rtk kubectl apply -f deploy.yaml"

echo ""

# ---- SECTION 4: Vitest edge case (fixed double "run" bug) ----
echo "--- Vitest run dedup ---"
test_rewrite "vitest (no args)" \
  "vitest" \
  "rtk vitest run"

test_rewrite "vitest run (no double run)" \
  "vitest run" \
  "rtk vitest run"

test_rewrite "vitest run --reporter" \
  "vitest run --reporter=verbose" \
  "rtk vitest run --reporter=verbose"

test_rewrite "npx vitest run" \
  "npx vitest run" \
  "rtk vitest run"

test_rewrite "pnpm vitest run --coverage" \
  "pnpm vitest run --coverage" \
  "rtk vitest run --coverage"

echo ""

# ---- SECTION 5: Should NOT rewrite ----
echo "--- Should NOT rewrite ---"
test_rewrite "already rtk" \
  "rtk git status" \
  ""

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

test_rewrite "python3 (no pattern)" \
  "python3 script.py" \
  ""

test_rewrite "node (no pattern)" \
  "node -e 'console.log(1)'" \
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
