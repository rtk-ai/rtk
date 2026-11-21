#!/usr/bin/env bash
# Exercise the security-guardian's documented hook with isolated RTK executables.
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd)
TEST_DIR=$(mktemp -d)
trap 'rm -rf -- "$TEST_DIR"' EXIT
HOOK="$TEST_DIR/hook.sh"
PASS=0
FAIL=0

# Execute the example itself so the tests cannot drift from the documented check.
awk '
    { sub(/\r$/, "") }
    /^\*\*Hook security checklist\*\*:/ { checklist = 1; next }
    checklist && /^```bash$/ { in_hook = 1; next }
    in_hook && /^```$/ { exit }
    in_hook { print }
' "$REPO_ROOT/.claude/skills/security-guardian/SKILL.md" > "$HOOK"
if [[ ! -s "$HOOK" ]]; then
    echo "FAIL: security-guardian hook example not found" >&2
    exit 1
fi

mkdir -p "$TEST_DIR/bin with spaces" "$TEST_DIR/empty"
cat > "$TEST_DIR/bin with spaces/rtk" <<'MOCK'
#!/bin/bash
case "${1-}" in
    --version)
        printf '%s\n' "$MOCK_VERSION"
        exit "$MOCK_VERSION_STATUS"
        ;;
    gain)
        printf '%s\n' "$MOCK_HELP"
        exit "$MOCK_HELP_STATUS"
        ;;
    *)
        printf '%s\n' "$@" > "$MOCK_EXECUTED"
        exit "$MOCK_COMMAND_STATUS"
        ;;
esac
MOCK
chmod +x "$TEST_DIR/bin with spaces/rtk"

record() {
    if [[ "$1" == pass ]]; then
        PASS=$((PASS + 1))
        printf '  PASS: %s\n' "$2"
    else
        FAIL=$((FAIL + 1))
        printf '  FAIL: %s\n' "$2"
    fi
}

check_hook() {
    local name=$1 expected_status=$2 expected_execution=$3 status=0
    local case_path="$TEST_DIR/bin with spaces:/usr/bin:/bin"
    [[ "${MOCK_AVAILABLE:-1}" == 1 ]] || case_path="$TEST_DIR/empty"
    rm -f "$TEST_DIR/executed"
    PATH="$case_path" \
        CLAUDE_CODE_HOOK_BASH_TEMPLATE="${HOOK_CONTEXT-1}" \
        MOCK_VERSION="${MOCK_VERSION-rtk 1.0.1}" \
        MOCK_VERSION_STATUS="${MOCK_VERSION_STATUS:-0}" \
        MOCK_HELP="${MOCK_HELP-$'Show token savings summary and history\n\nUsage: rtk gain [OPTIONS]'}" \
        MOCK_HELP_STATUS="${MOCK_HELP_STATUS:-0}" \
        MOCK_COMMAND_STATUS="${MOCK_COMMAND_STATUS:-0}" \
        MOCK_EXECUTED="$TEST_DIR/executed" \
        "$BASH" "$HOOK" verified-command 'argument with spaces' \
        > "$TEST_DIR/stdout" 2> "$TEST_DIR/stderr" || status=$?

    local executed=no
    [[ ! -f "$TEST_DIR/executed" ]] || executed=yes
    if [[ "$status" != "$expected_status" || "$executed" != "$expected_execution" ]]; then
        record fail "$name (status=$status, executed=$executed)"
        cat "$TEST_DIR/stderr"
    elif [[ "$expected_execution" == yes ]] &&
        { [[ -s "$TEST_DIR/stdout" || -s "$TEST_DIR/stderr" ]] ||
          [[ "$(cat "$TEST_DIR/executed")" != $'verified-command\nargument with spaces' ]]; }; then
        record fail "$name (unexpected warning or altered arguments)"
    elif [[ "$expected_execution" == no && "${HOOK_CONTEXT-1}" != '' ]] &&
        ! grep -q '^Warning:' "$TEST_DIR/stderr"; then
        record fail "$name (missing diagnostic)"
    else
        record pass "$name"
    fi
}

CURRENT_VERSION=$(awk '
    /^\[package\]/ { package = 1; next }
    package && /^version = / { gsub(/"|\r/, "", $3); print $3; exit }
' "$REPO_ROOT/Cargo.toml")
[[ -n "$CURRENT_VERSION" ]]
MOCK_VERSION="rtk $CURRENT_VERSION" check_hook 'current package version' 0 yes
MOCK_VERSION='rtk 99.10.11' check_hook 'future release' 0 yes
MOCK_VERSION='rtk 0.16.0' check_hook 'older release without an invented minimum' 0 yes
MOCK_VERSION='rtk 2.0.0-rc.1+build.42' check_hook 'prerelease and build metadata' 0 yes
MOCK_HELP=$'Show token savings summary and history\n\nUsage: rtk.exe gain [OPTIONS]' check_hook 'Windows executable help' 0 yes
MOCK_VERSION='other 1.0.0' check_hook 'unrelated executable name' 1 no
MOCK_HELP=$'Rust Type Kit\nUsage: rtk [OPTIONS]' check_hook 'same-name unrelated package' 1 no
MOCK_HELP='Usage: rtk gainful [OPTIONS]' check_hook 'different subcommand name' 1 no
MOCK_HELP='' check_hook 'empty identity help' 1 no
MOCK_HELP_STATUS=2 check_hook 'failed identity command with plausible output' 1 no
MOCK_VERSION_STATUS=2 check_hook 'failed version command with plausible output' 1 no
for version in '' 'rtk' 'rtk 1.2' 'rtk 1.2.3 garbage' 'garbage rtk 1.2.3' $'rtk 1.2.3\nother output'; do
    MOCK_VERSION="$version" check_hook "malformed version: $(printf '%q' "$version")" 1 no
done
MOCK_AVAILABLE=0 check_hook 'missing executable' 1 no
HOOK_CONTEXT='' check_hook 'missing Claude hook context' 1 no
MOCK_COMMAND_STATUS=17 check_hook 'preserve checked command exit status' 17 yes

for readme in README.md README_{es,fr,ja,ko,pt,zh}.md; do
    if grep -Eq 'rtk --version.*rtk [0-9]+\.[0-9]+' "$REPO_ROOT/$readme" ||
        ! grep -Eq '^rtk gain[[:space:]]' "$REPO_ROOT/$readme"; then
        record fail "$readme installation verification"
    else
        record pass "$readme installation verification"
    fi
done

printf '\nVersion verification: %s passed, %s failed\n' "$PASS" "$FAIL"
[[ "$FAIL" == 0 ]]
