#!/usr/bin/env bash
set -euo pipefail

# check-test-presence.sh — CI guard: new/modified *_cmd.rs files must have #[cfg(test)]
#
# Usage:
#   bash scripts/check-test-presence.sh [BASE_BRANCH]
#   bash scripts/check-test-presence.sh --self-test
#
# BASE_BRANCH defaults to origin/develop

if [ "${1:-}" = "--self-test" ]; then
    # Self-test: create a tempfile without tests and verify the check catches it
    TMPFILE="src/cmds/system/_rtk_check_self_test_cmd.rs"
    echo "pub fn run() {}" > "$TMPFILE"
    trap 'rm -f "$TMPFILE"' EXIT

    if grep -q '#\[cfg(test)\]' "$TMPFILE"; then
        echo "FAIL: self-test broken (false negative)"
        exit 1
    fi
    rm "$TMPFILE"
    trap - EXIT
    echo "PASS: --self-test detection works correctly"
    exit 0
fi

BASE_BRANCH="${1:-origin/develop}"
EXIT_CODE=0

# Find *_cmd.rs files that were added or modified in this PR
CHANGED_FILES=$(git diff --name-only --diff-filter=AM --no-renames "$BASE_BRANCH"...HEAD \
    2>/dev/null | grep -E 'src/cmds/.+_cmd\.rs$' || true)

if [ -z "$CHANGED_FILES" ]; then
    echo "check-test-presence: no *_cmd.rs changes detected — OK"
    exit 0
fi

echo "check-test-presence: checking $(echo "$CHANGED_FILES" | wc -l | tr -d ' ') filter module(s)..."
echo ""

while IFS= read -r file; do
    if [ ! -f "$file" ]; then
        continue
    fi

    if grep -q '#\[cfg(test)\]' "$file"; then
        echo "  PASS  $file"
    else
        echo "  FAIL  $file"
        echo "        Missing #[cfg(test)] module."
        echo "        Every *_cmd.rs filter must include inline unit tests."
        echo "        Reference: src/cmds/cloud/aws_cmd.rs"
        echo ""
        EXIT_CODE=1
    fi
done <<< "$CHANGED_FILES"

echo ""

if [ "$EXIT_CODE" -ne 0 ]; then
    echo "check-test-presence: FAILED — add tests before merging."
    echo "See .claude/rules/cli-testing.md for the testing guide."
else
    echo "check-test-presence: all filter modules have tests — OK"
fi

exit "$EXIT_CODE"
