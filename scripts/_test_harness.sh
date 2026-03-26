#!/usr/bin/env bash
# Shared test harness for RTK smoke test scripts.
# Source this file at the top of each test script:
#   source "$(dirname "$0")/_test_harness.sh"

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

# Allow non-zero exit but check output (case-insensitive)
assert_output() {
    local name="$1"; local needle="$2"; shift 2
    local output
    output=$("$@" 2>&1) || true
    if echo "$output" | grep -qi "$needle"; then
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

# Assert command exits with non-zero and output matches needle (case-insensitive)
assert_exit_nonzero() {
    local name="$1"; local needle="$2"; shift 2
    local output
    local rc=0
    output=$("$@" 2>&1) || rc=$?
    if [[ $rc -ne 0 ]] && echo "$output" | grep -qi "$needle"; then
        PASS=$((PASS + 1))
        printf "  ${GREEN}PASS${NC}  %s (exit=%d)\n" "$name" "$rc"
    else
        FAIL=$((FAIL + 1))
        FAILURES+=("$name")
        printf "  ${RED}FAIL${NC}  %s (exit=%d)\n" "$name" "$rc"
        if [[ $rc -eq 0 ]]; then
            printf "        expected non-zero exit, got 0\n"
        else
            printf "        expected: '%s'\n" "$needle"
        fi
        printf "        out: %s\n" "$(echo "$output" | head -3)"
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

report_and_exit() {
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
}
