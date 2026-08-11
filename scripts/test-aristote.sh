#!/usr/bin/env bash
#
# RTK Smoke Tests — Aristote Project (Vite + React + TS + ESLint)
# Tests RTK commands in a real JS/TS project context.
# Usage: bash scripts/test-aristote.sh
#
set -euo pipefail

ARISTOTE="/Users/florianbruniaux/Sites/MethodeAristote/aristote-school-boost"

PASS=0
FAIL=0
SKIP=0
FAILURES=()

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

assert_ok() {
    local name="$1"; shift
    local output
    if output=$("$@" 2>&1); then
        PASS=$((PASS + 1))
        printf "  ${GREEN}PASS${NC}  %s\n" "$name"
    else
        FAIL=$((FAIL + 1))
        FAILURES+=("$name")
        printf "  ${RED}FAIL${NC}  %s\n" "$name"
        printf "        cmd: %s\n" "$*"
        printf "        out: %s\n" "$(echo "$output" | head -3)"
    fi
}

assert_contains() {
    local name="$1"; local needle="$2"; shift 2
    local output
    if output=$("$@" 2>&1) && echo "$output" | grep -q "$needle"; then
        PASS=$((PASS + 1))
        printf "  ${GREEN}PASS${NC}  %s\n" "$name"
    else
        FAIL=$((FAIL + 1))
        FAILURES+=("$name")
        printf "  ${RED}FAIL${NC}  %s\n" "$name"
        printf "        expected: '%s'\n" "$needle"
        printf "        got: %s\n" "$(echo "$output" | head -3)"
    fi
}

# Allow non-zero exit but check output
assert_output() {
    local name="$1"; local needle="$2"; shift 2
    local output
    output=$("$@" 2>&1) || true
    if echo "$output" | grep -q "$needle"; then
        PASS=$((PASS + 1))
        printf "  ${GREEN}PASS${NC}  %s\n" "$name"
    else
        FAIL=$((FAIL + 1))
        FAILURES+=("$name")
        printf "  ${RED}FAIL${NC}  %s\n" "$name"
        printf "        expected: '%s'\n" "$needle"
        printf "        got: %s\n" "$(echo "$output" | head -3)"
    fi
}

skip_test() {
    local name="$1"; local reason="$2"
    SKIP=$((SKIP + 1))
    printf "  ${YELLOW}SKIP${NC}  %s (%s)\n" "$name" "$reason"
}

section() {
    printf "\n${BOLD}${CYAN}── %s ──${NC}\n" "$1"
}

# ── Preamble ─────────────────────────────────────────

RTK=$(command -v rtk || echo "")
if [[ -z "$RTK" ]]; then
    echo "rtk not found in PATH. Run: cargo install --path ."
    exit 1
fi

if [[ ! -d "$ARISTOTE" ]]; then
    echo "Aristote project not found at $ARISTOTE"
    exit 1
fi

printf "${BOLD}RTK Smoke Tests — Aristote Project${NC}\n"
printf "Binary: %s (%s)\n" "$RTK" "$(rtk --version)"
printf "Project: %s\n" "$ARISTOTE"
printf "Date: %s\n\n" "$(date '+%Y-%m-%d %H:%M')"

# ── 1. File exploration ──────────────────────────────

section "Ls & Find"

assert_ok       "rtk ls project root"           rtk ls "$ARISTOTE"
assert_ok       "rtk ls src/"                   rtk ls "$ARISTOTE/src"
assert_ok       "rtk ls --depth 3"              rtk ls --depth 3 "$ARISTOTE/src"
assert_contains "rtk ls shows components/"      "components" rtk ls "$ARISTOTE/src"
assert_ok       "rtk find *.tsx"                rtk find "*.tsx" "$ARISTOTE/src"
assert_ok       "rtk find *.ts"                 rtk find "*.ts" "$ARISTOTE/src"
assert_contains "rtk find finds App.tsx"        "App.tsx" rtk find "*.tsx" "$ARISTOTE/src"

# ── 2. Read ──────────────────────────────────────────

section "Read"

assert_ok       "rtk read tsconfig.json"        rtk read "$ARISTOTE/tsconfig.json"
assert_ok       "rtk read package.json"         rtk read "$ARISTOTE/package.json"
assert_ok       "rtk read App.tsx"              rtk read "$ARISTOTE/src/App.tsx"
assert_ok       "rtk read --level aggressive"   rtk read --level aggressive "$ARISTOTE/src/App.tsx"
assert_ok       "rtk read --max-lines 10"       rtk read --max-lines 10 "$ARISTOTE/src/App.tsx"

# ── 3. Grep ──────────────────────────────────────────

section "Grep"

assert_ok       "rtk grep import"               rtk grep "import" "$ARISTOTE/src"
assert_ok       "rtk grep with type filter"     rtk grep "useState" "$ARISTOTE/src" -t tsx
assert_contains "rtk grep finds components"     "import" rtk grep "import" "$ARISTOTE/src"

# ── 4. Git ───────────────────────────────────────────

section "Git (in Aristote repo)"

# rtk git doesn't support -C, use git -C via subshell
assert_ok       "rtk git status"                bash -c "cd $ARISTOTE && rtk git status"
assert_ok       "rtk git log"                   bash -c "cd $ARISTOTE && rtk git log"
assert_ok       "rtk git branch"                bash -c "cd $ARISTOTE && rtk git branch"

# ── 5. Deps ──────────────────────────────────────────

section "Deps"

assert_ok       "rtk deps"                      rtk deps "$ARISTOTE"
assert_contains "rtk deps shows package.json"   "package.json" rtk deps "$ARISTOTE"

# ── 6. Json ──────────────────────────────────────────

section "Json"

assert_ok       "rtk json tsconfig"             rtk json "$ARISTOTE/tsconfig.json"
assert_ok       "rtk json package.json"         rtk json "$ARISTOTE/package.json"

# ── 7. Env ───────────────────────────────────────────

section "Env"

assert_ok       "rtk env"                       rtk env
assert_ok       "rtk env --filter NODE"         rtk env --filter NODE

# ── 8. Tsc ───────────────────────────────────────────

section "TypeScript (tsc)"

if command -v npx >/dev/null 2>&1 && [[ -d "$ARISTOTE/node_modules" ]]; then
    assert_output "rtk tsc (in aristote)" "error\|✅\|TS" rtk tsc --project "$ARISTOTE"
else
    skip_test "rtk tsc" "node_modules not installed"
fi

# ── 9. ESLint ────────────────────────────────────────

section "ESLint (lint)"

if command -v npx >/dev/null 2>&1 && [[ -d "$ARISTOTE/node_modules" ]]; then
    assert_output "rtk lint (in aristote)" "error\|warning\|✅\|violations\|clean" rtk lint --project "$ARISTOTE"
else
    skip_test "rtk lint" "node_modules not installed"
fi

# ── 10. Build (Vite) ─────────────────────────────────

section "Build (Vite via rtk next)"

if [[ -d "$ARISTOTE/node_modules" ]]; then
    # Aristote uses Vite, not Next — but rtk next wraps the build script
    # Test with a timeout since builds can be slow
    skip_test "rtk next build" "Vite project, not Next.js — use npm run build directly"
else
    skip_test "rtk next build" "node_modules not installed"
fi

# ── 11. Diff ─────────────────────────────────────────

section "Diff"

# Diff two config files that exist in the project
assert_ok       "rtk diff tsconfigs"            rtk diff "$ARISTOTE/tsconfig.json" "$ARISTOTE/tsconfig.app.json"

# ── 12. Summary & Err ────────────────────────────────

section "Summary & Err"

assert_ok       "rtk summary ls"                rtk summary ls "$ARISTOTE/src"
assert_ok       "rtk err ls"                    rtk err ls "$ARISTOTE/src"

# ── 13. Gain ─────────────────────────────────────────

section "Gain (after above commands)"

assert_ok       "rtk gain"                      rtk gain
assert_ok       "rtk gain --history"            rtk gain --history

# ══════════════════════════════════════════════════════
# Report
# ══════════════════════════════════════════════════════

printf "\n${BOLD}══════════════════════════════════════${NC}\n"
printf "${BOLD}Results: ${GREEN}%d passed${NC}, ${RED}%d failed${NC}, ${YELLOW}%d skipped${NC}\n" "$PASS" "$FAIL" "$SKIP"

if [[ ${#FAILURES[@]} -gt 0 ]]; then
    printf "\n${RED}Failures:${NC}\n"
    for f in "${FAILURES[@]}"; do
        printf "  - %s\n" "$f"
    done
fi

printf "${BOLD}══════════════════════════════════════${NC}\n"

exit "$FAIL"
