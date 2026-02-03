#!/usr/bin/env bash
#
# RTK Smoke Test Suite
# Exercises every command to catch regressions after merge.
# Exit code: number of failures (0 = all green)
#
set -euo pipefail

PASS=0
FAIL=0
SKIP=0
FAILURES=()

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# ── Helpers ──────────────────────────────────────────

assert_ok() {
    local name="$1"
    shift
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
    local name="$1"
    local needle="$2"
    shift 2
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

assert_exit_ok() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        PASS=$((PASS + 1))
        printf "  ${GREEN}PASS${NC}  %s\n" "$name"
    else
        FAIL=$((FAIL + 1))
        FAILURES+=("$name")
        printf "  ${RED}FAIL${NC}  %s\n" "$name"
        printf "        cmd: %s\n" "$*"
    fi
}

assert_help() {
    local name="$1"
    shift
    assert_contains "$name --help" "Usage:" "$@" --help
}

skip_test() {
    local name="$1"
    local reason="$2"
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

printf "${BOLD}RTK Smoke Test Suite${NC}\n"
printf "Binary: %s\n" "$RTK"
printf "Version: %s\n" "$(rtk --version)"
printf "Date: %s\n" "$(date '+%Y-%m-%d %H:%M')"

# Need a git repo to test git commands
if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "Must run from inside a git repository."
    exit 1
fi

REPO_ROOT=$(git rev-parse --show-toplevel)

# ── 1. Version & Help ───────────────────────────────

section "Version & Help"

assert_contains "rtk --version" "rtk" rtk --version
assert_contains "rtk --help" "Usage:" rtk --help

# ── 2. Ls ────────────────────────────────────────────

section "Ls"

assert_ok      "rtk ls ."                     rtk ls .
assert_ok      "rtk ls -la ."                 rtk ls -la .
assert_ok      "rtk ls -lh ."                 rtk ls -lh .
assert_ok      "rtk ls -l src/"               rtk ls -l src/
assert_ok      "rtk ls src/ -l (flag after)"  rtk ls src/ -l
assert_ok      "rtk ls multi paths"           rtk ls src/ scripts/
assert_contains "rtk ls -a shows hidden"      ".git" rtk ls -a .
assert_contains "rtk ls shows sizes"          "K"  rtk ls src/
assert_contains "rtk ls shows dirs with /"    "/" rtk ls .

# ── 2b. Tree ─────────────────────────────────────────

section "Tree"

if command -v tree >/dev/null 2>&1; then
    assert_ok      "rtk tree ."                rtk tree .
    assert_ok      "rtk tree -L 2 ."           rtk tree -L 2 .
    assert_ok      "rtk tree -d -L 1 ."        rtk tree -d -L 1 .
    assert_contains "rtk tree shows src/"      "src" rtk tree -L 1 .
else
    skip_test "rtk tree" "tree not installed"
fi

# ── 3. Read ──────────────────────────────────────────

section "Read"

assert_ok      "rtk read Cargo.toml"          rtk read Cargo.toml
assert_ok      "rtk read --level none Cargo.toml"  rtk read --level none Cargo.toml
assert_ok      "rtk read --level aggressive Cargo.toml" rtk read --level aggressive Cargo.toml
assert_ok      "rtk read -n Cargo.toml"       rtk read -n Cargo.toml
assert_ok      "rtk read --max-lines 5 Cargo.toml" rtk read --max-lines 5 Cargo.toml

section "Read (stdin support)"

assert_ok      "rtk read stdin pipe"          bash -c 'echo "fn main() {}" | rtk read -'

# ── 4. Git ───────────────────────────────────────────

section "Git (existing)"

assert_ok      "rtk git status"               rtk git status
assert_ok      "rtk git status --short"       rtk git status --short
assert_ok      "rtk git status -s"            rtk git status -s
assert_ok      "rtk git status --porcelain"   rtk git status --porcelain
assert_ok      "rtk git log"                  rtk git log
assert_ok      "rtk git log -5"               rtk git log -- -5
assert_ok      "rtk git diff"                 rtk git diff
assert_ok      "rtk git diff --stat"          rtk git diff --stat

section "Git (new: branch, fetch, stash, worktree)"

assert_ok      "rtk git branch"               rtk git branch
assert_ok      "rtk git fetch"                rtk git fetch
assert_ok      "rtk git stash list"           rtk git stash list
assert_ok      "rtk git worktree"             rtk git worktree

section "Git (passthrough: unsupported subcommands)"

assert_ok      "rtk git tag --list"           rtk git tag --list
assert_ok      "rtk git remote -v"            rtk git remote -v
assert_ok      "rtk git rev-parse HEAD"       rtk git rev-parse HEAD

# ── 5. GitHub CLI ────────────────────────────────────

section "GitHub CLI"

if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
    assert_ok      "rtk gh pr list"           rtk gh pr list
    assert_ok      "rtk gh run list"          rtk gh run list
    assert_ok      "rtk gh issue list"        rtk gh issue list
    # pr create/merge/diff/comment/edit are write ops, test help only
    assert_help    "rtk gh"                   rtk gh
else
    skip_test "gh commands" "gh not authenticated"
fi

# ── 6. Cargo ─────────────────────────────────────────

section "Cargo (new)"

assert_ok      "rtk cargo build"              rtk cargo build
assert_ok      "rtk cargo clippy"             rtk cargo clippy
# cargo test exits non-zero due to pre-existing failures; check output ignoring exit code
output_cargo_test=$(rtk cargo test 2>&1 || true)
if echo "$output_cargo_test" | grep -q "FAILURES\|test result:"; then
    PASS=$((PASS + 1))
    printf "  ${GREEN}PASS${NC}  %s\n" "rtk cargo test"
else
    FAIL=$((FAIL + 1))
    FAILURES+=("rtk cargo test")
    printf "  ${RED}FAIL${NC}  %s\n" "rtk cargo test"
    printf "        got: %s\n" "$(echo "$output_cargo_test" | head -3)"
fi
assert_help    "rtk cargo"                    rtk cargo

# ── 7. Curl ──────────────────────────────────────────

section "Curl (new)"

assert_contains "rtk curl JSON detect" "string" rtk curl https://httpbin.org/json
assert_ok       "rtk curl plain text"          rtk curl https://httpbin.org/robots.txt
assert_help     "rtk curl"                     rtk curl

# ── 8. Npm / Npx ────────────────────────────────────

section "Npm / Npx (new)"

assert_help    "rtk npm"                      rtk npm
assert_help    "rtk npx"                      rtk npx

# ── 9. Pnpm ─────────────────────────────────────────

section "Pnpm"

assert_help    "rtk pnpm"                     rtk pnpm
assert_help    "rtk pnpm build"               rtk pnpm build
assert_help    "rtk pnpm typecheck"           rtk pnpm typecheck

if command -v pnpm >/dev/null 2>&1; then
    assert_ok  "rtk pnpm help"                rtk pnpm help
fi

# ── 10. Grep ─────────────────────────────────────────

section "Grep"

assert_ok      "rtk grep pattern"             rtk grep "pub fn" src/
assert_contains "rtk grep finds results"      "pub fn" rtk grep "pub fn" src/
assert_ok      "rtk grep with file type"      rtk grep "pub fn" src/ -t rust

section "Grep (extra args passthrough)"

assert_ok      "rtk grep -i case insensitive" rtk grep "fn" src/ -i
assert_ok      "rtk grep -A context lines"    rtk grep "fn run" src/ -A 2

# ── 11. Find ─────────────────────────────────────────

section "Find"

assert_ok      "rtk find *.rs"                rtk find "*.rs" src/
assert_contains "rtk find shows files"        ".rs" rtk find "*.rs" src/

# ── 12. Json ─────────────────────────────────────────

section "Json"

# Create temp JSON file for testing
TMPJSON=$(mktemp /tmp/rtk-test-XXXXX.json)
echo '{"name":"test","count":42,"items":[1,2,3]}' > "$TMPJSON"

assert_ok      "rtk json file"                rtk json "$TMPJSON"
assert_contains "rtk json shows schema"       "string" rtk json "$TMPJSON"

rm -f "$TMPJSON"

# ── 13. Deps ─────────────────────────────────────────

section "Deps"

assert_ok      "rtk deps ."                   rtk deps .
assert_contains "rtk deps shows Cargo"        "Cargo" rtk deps .

# ── 14. Env ──────────────────────────────────────────

section "Env"

assert_ok      "rtk env"                      rtk env
assert_ok      "rtk env --filter PATH"        rtk env --filter PATH

# ── 16. Log ──────────────────────────────────────────

section "Log"

TMPLOG=$(mktemp /tmp/rtk-log-XXXXX.log)
for i in $(seq 1 20); do
    echo "[2025-01-01 12:00:00] INFO: repeated message" >> "$TMPLOG"
done
echo "[2025-01-01 12:00:01] ERROR: something failed" >> "$TMPLOG"

assert_ok      "rtk log file"                 rtk log "$TMPLOG"

rm -f "$TMPLOG"

# ── 17. Summary ──────────────────────────────────────

section "Summary"

assert_ok      "rtk summary echo hello"       rtk summary echo hello

# ── 18. Err ──────────────────────────────────────────

section "Err"

assert_ok      "rtk err echo ok"              rtk err echo ok

# ── 19. Test runner ──────────────────────────────────

section "Test runner"

assert_ok      "rtk test echo ok"             rtk test echo ok

# ── 20. Gain ─────────────────────────────────────────

section "Gain"

assert_ok      "rtk gain"                     rtk gain
assert_ok      "rtk gain --history"           rtk gain --history

# ── 21. Config & Init ────────────────────────────────

section "Config & Init"

assert_ok      "rtk config"                   rtk config
assert_ok      "rtk init --show"              rtk init --show

# ── 22. Wget ─────────────────────────────────────────

section "Wget"

if command -v wget >/dev/null 2>&1; then
    assert_ok  "rtk wget stdout"              rtk wget https://httpbin.org/robots.txt -O
else
    skip_test "rtk wget" "wget not installed"
fi

# ── 23. Tsc / Lint / Prettier / Next / Playwright ───

section "JS Tooling (help only, no project context)"

assert_help    "rtk tsc"                      rtk tsc
assert_help    "rtk lint"                     rtk lint
assert_help    "rtk prettier"                 rtk prettier
assert_help    "rtk next"                     rtk next
assert_help    "rtk playwright"               rtk playwright

# ── 24. Prisma ───────────────────────────────────────

section "Prisma (help only)"

assert_help    "rtk prisma"                   rtk prisma

# ── 25. Vitest ───────────────────────────────────────

section "Vitest (help only)"

assert_help    "rtk vitest"                   rtk vitest

# ── 26. Docker / Kubectl (help only) ────────────────

section "Docker / Kubectl (help only)"

assert_help    "rtk docker"                   rtk docker
assert_help    "rtk kubectl"                  rtk kubectl

# ── 27. Global flags ────────────────────────────────

section "Global flags"

assert_ok      "rtk -u ls ."                  rtk -u ls .
assert_ok      "rtk --skip-env npm --help"    rtk --skip-env npm --help

# ── 28. CcEconomics ─────────────────────────────────

section "CcEconomics"

assert_ok      "rtk cc-economics"             rtk cc-economics

# ── 29. Learn ───────────────────────────────────────

section "Learn"

assert_ok      "rtk learn --help"             rtk learn --help
assert_ok      "rtk learn (no sessions)"      rtk learn --since 0 2>&1 || true

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
