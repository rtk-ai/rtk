#!/usr/bin/env bash
# Test tracking end-to-end: run commands, verify they appear in rtk gain --history
set -euo pipefail

# Workaround for macOS bash pipe handling in strict mode
set +e  # Allow errors in pipe chains to continue

PASS=0; FAIL=0; FAILURES=()
RED='\033[0;31m'; GREEN='\033[0;32m'; NC='\033[0m'

check() {
    local name="$1" needle="$2"
    shift 2
    local output
    if output=$("$@" 2>&1) && echo "$output" | grep -q "$needle"; then
        PASS=$((PASS+1)); printf "  ${GREEN}PASS${NC}  %s\n" "$name"
    else
        FAIL=$((FAIL+1)); FAILURES+=("$name")
        printf "  ${RED}FAIL${NC}  %s\n" "$name"
        printf "        expected: '%s'\n" "$needle"
        printf "        got: %s\n" "$(echo "$output" | head -3)"
    fi
}

echo "═══ RTK Tracking Validation ═══"
echo ""

# 1. Commandes avec filtrage réel — doivent apparaitre dans history
echo "── Optimized commands (token savings) ──"
rtk ls . >/dev/null 2>&1
check "rtk ls tracked" "rtk ls" rtk gain --history

rtk git status >/dev/null 2>&1
check "rtk git status tracked" "rtk git status" rtk gain --history

rtk git log -5 >/dev/null 2>&1
check "rtk git log tracked" "rtk git log" rtk gain --history

# Git passthrough (timing-only)
echo ""
echo "── Passthrough commands (timing-only) ──"
rtk git tag --list >/dev/null 2>&1
check "git passthrough tracked" "git tag --list" rtk gain --history

# gh commands (if authenticated)
echo ""
echo "── GitHub CLI tracking ──"
if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
    rtk gh pr list >/dev/null 2>&1 || true
    check "rtk gh pr list tracked" "rtk gh pr" rtk gain --history

    rtk gh run list >/dev/null 2>&1 || true
    check "rtk gh run list tracked" "rtk gh run" rtk gain --history
else
    echo "  SKIP  gh (not authenticated)"
fi

# Stdin commands
echo ""
echo "── Stdin commands ──"
echo -e "line1\nline2\nline1\nERROR: bad\nline1" | rtk log >/dev/null 2>&1
check "rtk log stdin tracked" "rtk log" rtk gain --history

# Summary — verify passthrough doesn't dilute
echo ""
echo "── Summary integrity ──"
output=$(rtk gain 2>&1)
if echo "$output" | grep -q "Tokens saved"; then
    PASS=$((PASS+1)); printf "  ${GREEN}PASS${NC}  rtk gain summary works\n"
else
    FAIL=$((FAIL+1)); printf "  ${RED}FAIL${NC}  rtk gain summary\n"
fi

echo ""
echo "═══ Results: ${PASS} passed, ${FAIL} failed ═══"
if [ ${#FAILURES[@]} -gt 0 ]; then
    echo "Failures: ${FAILURES[*]}"
fi
exit $FAIL
