#!/usr/bin/env bash
#
# RTK Smoke Tests — Bundle + Rails (TOML/Rust hybrid)
# Creates a minimal Rails app, exercises RTK bundle/rails filters, then cleans up.
# Bundle commands use TOML DSL filters (via fallback).
# Rails test/routes use Rust filters; db:migrate/rollback/generate use TOML (via run_other).
# Usage: bash scripts/test-bundle-rails.sh
#
# Prerequisites: rtk, ruby, bundler, rails gem
# Duration: ~60-120s (rails new + bundle install dominate)
#
set -euo pipefail

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

# ── Helpers ──────────────────────────────────────────

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

skip_test() {
    local name="$1"; local reason="$2"
    SKIP=$((SKIP + 1))
    printf "  ${YELLOW}SKIP${NC}  %s (%s)\n" "$name" "$reason"
}

# Assert command exits with non-zero and output matches needle
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

section() {
    printf "\n${BOLD}${CYAN}── %s ──${NC}\n" "$1"
}

# ── Prerequisite checks ─────────────────────────────

RTK=$(command -v rtk || echo "")
if [[ -z "$RTK" ]]; then
    echo "rtk not found in PATH. Run: cargo install --path ."
    exit 1
fi

if ! command -v ruby >/dev/null 2>&1; then
    echo "ruby not found in PATH. Install Ruby first."
    exit 1
fi

if ! command -v bundle >/dev/null 2>&1; then
    echo "bundler not found in PATH. Run: gem install bundler"
    exit 1
fi

if ! command -v rails >/dev/null 2>&1; then
    echo "rails not found in PATH. Run: gem install rails"
    exit 1
fi

# ── Preamble ─────────────────────────────────────────

printf "${BOLD}RTK Smoke Tests — Bundle + Rails${NC}\n"
printf "Binary: %s (%s)\n" "$RTK" "$(rtk --version)"
printf "Ruby: %s\n" "$(ruby --version)"
printf "Rails: %s\n" "$(rails --version)"
printf "Bundler: %s\n" "$(bundle --version)"
printf "Date: %s\n\n" "$(date '+%Y-%m-%d %H:%M')"

# ── Temp dir + cleanup trap ──────────────────────────

TMPDIR=$(mktemp -d /tmp/rtk-bundle-rails-smoke-XXXXXX)
trap 'rm -rf "$TMPDIR"' EXIT

printf "${BOLD}Setting up temporary Rails app in %s ...${NC}\n" "$TMPDIR"

# ── Setup phase (not counted in assertions) ──────────

cd "$TMPDIR"

# 1. Create minimal Rails app
printf "  → rails new (--minimal --skip-git --skip-docker) ...\n"
rails new rtk_smoke_app --minimal --skip-git --skip-docker --quiet 2>&1 | tail -1 || true
cd rtk_smoke_app

# 2. Bundle install
printf "  → bundle install ...\n"
bundle install --quiet 2>&1 | tail -1 || true

# 3. Generate scaffold (creates minitest tests in test/)
printf "  → rails generate scaffold Post ...\n"
rails generate scaffold Post title:string body:text published:boolean --quiet 2>&1 | tail -1 || true

# 4. Create + migrate database
printf "  → rails db:create && db:migrate ...\n"
rails db:create --quiet 2>&1 | tail -1 || true
rails db:migrate --quiet 2>&1 | tail -1 || true

# 5. Configure multi-DB (primary alias) for multi-DB variant tests
printf "  → configuring multi-DB (primary) ...\n"
cat > config/database.yml <<'DBYML'
default: &default
  adapter: sqlite3
  pool: 5
  timeout: 5000

development:
  primary:
    <<: *default
    database: storage/development.sqlite3

test:
  primary:
    <<: *default
    database: storage/test.sqlite3

production:
  primary:
    <<: *default
    database: storage/production.sqlite3
DBYML

# Re-create + migrate with multi-DB config
rails db:create --quiet 2>&1 || true
rails db:migrate --quiet 2>&1 || true

# 6. Create a failing minitest test
printf "  → creating failing minitest test ...\n"
cat > test/models/post_fail_test.rb <<'FAILTEST'
require "test_helper"

class PostFailTest < ActiveSupport::TestCase
  test "this test intentionally fails" do
    post = Post.new(title: nil, body: nil, published: nil)
    assert_equal "Expected Title", post.title, "Title should match but won't"
  end
end
FAILTEST

# 7. Create a passing-only minitest test (no failures)
printf "  → creating passing-only minitest test ...\n"
cat > test/models/post_pass_test.rb <<'PASSTEST'
require "test_helper"

class PostPassTest < ActiveSupport::TestCase
  test "title can be set" do
    post = Post.new(title: "Hello")
    assert_equal "Hello", post.title
  end

  test "body can be set" do
    post = Post.new(body: "World")
    assert_equal "World", post.body
  end
end
PASSTEST

printf "\n${BOLD}Setup complete. Running tests...${NC}\n"

# ══════════════════════════════════════════════════════
# Test sections
# ══════════════════════════════════════════════════════

# ── 1. rails generate ───────────────────────────────

section "Rails generate"

assert_output "rtk rails generate model Comment" \
    "create\|remove\|generate" \
    rtk rails generate model Comment post:references body:text

# Migrate the new model for later tests
rails db:migrate --quiet 2>&1 || true

# ── 2. rails db:migrate ─────────────────────────────

section "Rails db:migrate"

assert_output "rtk rails db:migrate (no-op)" \
    "db:migrate\|migrate\|already\|up to date\|no pending" \
    rtk rails db:migrate

# ── 3. rails db:migrate:status ──────────────────────

section "Rails db:migrate:status"

assert_output "rtk rails db:migrate:status" \
    "migration\|up\|down\|database" \
    rtk rails db:migrate:status

# ── 4. rails db:rollback ────────────────────────────

section "Rails db:rollback"

assert_output "rtk rails db:rollback" \
    "db:migrate\|rollback\|revert" \
    rtk rails db:rollback

# Re-migrate so later tests have all tables
rails db:migrate --quiet 2>&1 || true

# ── 5. rails test ───────────────────────────────────

section "Rails test (Minitest)"

assert_output "rtk rails test (with failure)" \
    "failed\|failure\|FAIL" \
    rtk rails test

# ── 6. rails routes ─────────────────────────────────

section "Rails routes"

assert_output "rtk rails routes" \
    "Routes\|route" \
    rtk rails routes

# ── 7. bundle list ──────────────────────────────────

section "Bundle"

assert_output "rtk bundle list (TOML filter)" \
    "\\*\|gems\|bundle" \
    rtk bundle list

assert_output "rtk bundle outdated (TOML filter)" \
    "bundle\|outdated\|up to date\|Gem\|Current" \
    rtk bundle outdated

assert_output "rtk bundle install (TOML filter, idempotent)" \
    "bundle\|ok\|complete\|install" \
    rtk bundle install

assert_output "rtk bundle update (TOML filter)" \
    "bundle\|ok\|complete\|update" \
    rtk bundle update

# ── 8. Multi-DB variants ─────────────────────────────

section "Multi-DB variants"

assert_output "rtk rails db:migrate:primary" \
    "migrate\|primary\|already" \
    rtk rails db:migrate:primary

assert_output "rtk rails db:rollback:primary" \
    "rollback\|revert\|migrate" \
    rtk rails db:rollback:primary

# Re-migrate after rollback
rails db:migrate --quiet 2>&1 || true

# ── 9. Exit code preservation ────────────────────────

section "Exit code preservation"

assert_exit_nonzero "rtk rails test exits non-zero on failure" \
    "failed\|failure\|FAIL" \
    rtk rails test

# ── 10. bundle exec variants ─────────────────────────

section "bundle exec variants"

assert_output "bundle exec rails routes" \
    "Routes\|route\|Prefix" \
    rtk bundle exec rails routes

# ── 11. bin/rails variants ────────────────────────────

section "bin/rails variants"

assert_output "bin/rails routes" \
    "Routes\|route\|Prefix" \
    rtk bin/rails routes

assert_output "bin/rails db:migrate:status" \
    "migration\|Status" \
    rtk bin/rails db:migrate:status

# ── 12. rake variants ────────────────────────────────

section "rake variants"

assert_output "rake routes" \
    "Routes\|route\|Prefix" \
    rtk rake routes

assert_output "rake db:migrate:status" \
    "migration\|Status" \
    rtk rake db:migrate:status

# ── 13. Rails passthrough ─────────────────────────────

section "Rails passthrough"

assert_output "rtk rails runner (passthrough)" \
    "42" \
    rtk rails runner "puts 42"

# ── 14. rails destroy ─────────────────────────────────

section "Rails destroy"

assert_output "rtk rails destroy model Comment (TOML filter)" \
    "remove\|destroy\|generate" \
    rtk rails destroy model Comment

# Re-migrate to clean up
rails db:migrate --quiet 2>&1 || true

# ── 15. Token savings ─────────────────────────────────

section "Token savings"

# rails routes
raw_len=$( (rails routes 2>&1 || true) | wc -c | tr -d ' ')
rtk_len=$( (rtk rails routes 2>&1 || true) | wc -c | tr -d ' ')
if [[ "$rtk_len" -lt "$raw_len" ]]; then
    PASS=$((PASS + 1))
    printf "  ${GREEN}PASS${NC}  rails routes: rtk (%s bytes) < raw (%s bytes)\n" "$rtk_len" "$raw_len"
else
    FAIL=$((FAIL + 1))
    FAILURES+=("token savings: rails routes")
    printf "  ${RED}FAIL${NC}  rails routes: rtk (%s bytes) >= raw (%s bytes)\n" "$rtk_len" "$raw_len"
fi

# ── 16. Rails passthrough (unknown subcommands) ──────

section "Rails passthrough (unknown subcommands)"

assert_output "rtk rails console --help (passthrough)" \
    "console\|Usage\|help\|IRB" \
    rtk rails console --help

assert_output "rtk rails server --help (passthrough)" \
    "server\|Usage\|help\|Puma\|port" \
    rtk rails server --help

# ── 17. rails test (passing only, no failures) ───────

section "Rails test (pass only)"

assert_output "rtk rails test passing file" \
    "passed\|✓" \
    rtk rails test test/models/post_pass_test.rb

# ── 18. rails routes -g (grep mode) ──────────────────

section "Rails routes grep"

assert_output "rtk rails routes -g posts" \
    "post\|POST\|route" \
    rtk rails routes -g posts

# ── 19. bundle passthrough (unknown subcommand) ──────

section "Bundle passthrough"

assert_output "bundle exec rake -T (passthrough)" \
    "rake\|task" \
    rtk bundle exec rake -T

# ── 20. rails test single file ────────────────────────

section "Rails test single file"

assert_output "rtk rails test single file (fail)" \
    "failed\|failure\|FAIL" \
    rtk rails test test/models/post_fail_test.rb

# ── 21. verbose flag ─────────────────────────────────

section "Verbose flag (-v)"

assert_output "rtk -v rails routes (verbose)" \
    "route\|Routes\|Running" \
    rtk -v rails routes

# ── 22. db:migrate:status all-up ──────────────────────

section "db:migrate:status (all up)"

# Make sure everything is migrated
rails db:migrate --quiet 2>&1 || true

assert_output "rtk rails db:migrate:status (all up, TOML filter)" \
    "all migrations up\|all up\|0 pending" \
    rtk rails db:migrate:status

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
