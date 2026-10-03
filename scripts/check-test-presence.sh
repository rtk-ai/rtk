#!/usr/bin/env bash
set -euo pipefail

# check-test-presence.sh — CI guard: new functions in *_cmd.rs must have test_<fn> coverage
#
# For each *_cmd.rs changed in this PR, parses newly added function signatures from the diff
# and verifies each non-trivial function has a corresponding fn test_<name>[_variant] in the file.
#
# A function is skipped if:
#   - It starts with test_ or _ (already a test or private helper)
#   - It's an entry point or trait impl (run, run_*, new, default, fmt, clone, ...)
#   - Its definition only appears inside the #[cfg(test)] block (test-module helpers)
#
# Usage:
#   bash scripts/check-test-presence.sh [BASE_BRANCH]
#   bash scripts/check-test-presence.sh --self-test
#
# BASE_BRANCH defaults to origin/develop

# fn_needs_test FNAME — returns 0 if a test is required, 1 if the function can be skipped.
fn_needs_test() {
    local name="$1"
    case "$name" in
        # Already a test or underscore-prefixed private helper
        test_* | _*) return 1 ;;
        # Entry points
        run | main) return 1 ;;
        # Constructors and standard trait methods
        new | default | clone | fmt | drop | eq | hash | cmp | partial_eq | partial_cmp) return 1 ;;
        # Prefix patterns: run_*, from_*, into_*, serialize*, deserialize*
        run_* | from_* | into_* | serialize* | deserialize* | size_hint*) return 1 ;;
    esac
    return 0
}

# fn_is_outside_test_block FILE FNAME — returns 0 if fn is defined before #[cfg(test)].
# Functions only inside the test block (e.g. count_tokens helper) are skipped.
fn_is_outside_test_block() {
    local file="$1"
    local fn_name="$2"
    awk '/^#\[cfg\(test\)\]/{exit} {print}' "$file" \
        | grep -qE "fn[[:space:]]+${fn_name}[[:space:](<]"
}

if [ "${1:-}" = "--self-test" ]; then
    ok=true

    # 1. Validate skip list
    for name in run run_foo run_passthrough main new default from_str from_utf8 \
                into_string fmt clone hash eq cmp drop test_foo _helper \
                serialize deserialize; do
        if fn_needs_test "$name"; then
            echo "FAIL: '$name' should be skipped but was not"
            ok=false
        fi
    done
    for name in filter_output compact_url format_size parse_error mask_value \
                is_cloud_var extract_filename; do
        if ! fn_needs_test "$name"; then
            echo "FAIL: '$name' should require a test but was skipped"
            ok=false
        fi
    done

    # 2. Validate function name extraction from a diff line
    sample='+    pub fn compact_url(url: &str) -> String {'
    extracted=$(echo "$sample" | sed -E 's/^.*fn[[:space:]]+([a-z_][a-z0-9_]*).*/\1/')
    if [ "$extracted" = "compact_url" ]; then
        echo "  OK  extraction: 'pub fn compact_url(...)' -> '$extracted'"
    else
        echo "FAIL: extraction got '$extracted', expected 'compact_url'"
        ok=false
    fi

    if $ok; then
        echo "PASS: --self-test all checks passed"
        exit 0
    fi
    exit 1
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

FILE_COUNT=$(echo "$CHANGED_FILES" | wc -l | tr -d ' ')
echo "check-test-presence: scanning $FILE_COUNT filter module(s) for untested functions..."
echo ""

while IFS= read -r file; do
    if [ ! -f "$file" ]; then
        continue
    fi

    # Extract function names added in this diff.
    # grep 1: ^\+[^+] keeps added lines (single +), skips +++ header lines.
    # grep 2: matches fn declarations of any visibility or modifier.
    ADDED_FNS=$(git diff --unified=0 "$BASE_BRANCH"...HEAD -- "$file" 2>/dev/null \
        | grep -E '^\+[^+]' \
        | grep -E 'fn[[:space:]]+[a-z_][a-z0-9_]*[[:space:](<]' \
        | sed -E 's/^.*fn[[:space:]]+([a-z_][a-z0-9_]*).*/\1/' \
        | sort -u \
        || true)

    if [ -z "$ADDED_FNS" ]; then
        echo "  OK  $file  (no new functions)"
        continue
    fi

    while IFS= read -r fn_name; do
        # Skip entry points, trait impls, and existing test names
        if ! fn_needs_test "$fn_name"; then
            continue
        fi
        # Skip test-module helpers (functions only defined inside #[cfg(test)])
        if ! fn_is_outside_test_block "$file" "$fn_name"; then
            continue
        fi
        if grep -q "fn test_${fn_name}" "$file"; then
            echo "  PASS  $file::${fn_name}()"
        else
            echo "  FAIL  $file::${fn_name}()"
            echo "        Expected: fn test_${fn_name}[_variant] in $file"
            EXIT_CODE=1
        fi
    done <<< "$ADDED_FNS"
done <<< "$CHANGED_FILES"

echo ""

if [ "$EXIT_CODE" -ne 0 ]; then
    echo "check-test-presence: FAILED — add tests before merging."
    echo "Convention: fn my_func -> fn test_my_func[_variant]"
    echo "Reference:  .claude/rules/cli-testing.md"
else
    echo "check-test-presence: all new functions have test coverage — OK"
fi

exit "$EXIT_CODE"
