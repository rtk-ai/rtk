#!/usr/bin/env bash
#
# RTK Smoke Tests — Ruby on Rails (temp app)
# Creates a minimal Rails app, exercises all RTK Ruby/Rails filters, then cleans up.
# Usage: bash scripts/test-rails.sh
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

printf "${BOLD}RTK Smoke Tests — Ruby on Rails${NC}\n"
printf "Binary: %s (%s)\n" "$RTK" "$(rtk --version)"
printf "Ruby: %s\n" "$(ruby --version)"
printf "Rails: %s\n" "$(rails --version)"
printf "Bundler: %s\n" "$(bundle --version)"
printf "Date: %s\n\n" "$(date '+%Y-%m-%d %H:%M')"

# ── Temp dir + cleanup trap ──────────────────────────

TMPDIR=$(mktemp -d /tmp/rtk-rails-smoke-XXXXXX)
trap 'rm -rf "$TMPDIR"' EXIT

printf "${BOLD}Setting up temporary Rails app in %s ...${NC}\n" "$TMPDIR"

# ── Setup phase (not counted in assertions) ──────────

cd "$TMPDIR"

# 1. Create minimal Rails app
printf "  → rails new (--minimal --skip-git --skip-docker) ...\n"
rails new rtk_smoke_app --minimal --skip-git --skip-docker --quiet 2>&1 | tail -1 || true
cd rtk_smoke_app

# 2. Add rspec-rails and rubocop to Gemfile
cat >> Gemfile <<'GEMFILE'

group :development, :test do
  gem 'rspec-rails'
  gem 'rubocop', require: false
end
GEMFILE

# 3. Bundle install
printf "  → bundle install ...\n"
bundle install --quiet 2>&1 | tail -1 || true

# 4. Generate scaffold (creates minitest tests in test/)
printf "  → rails generate scaffold Post ...\n"
rails generate scaffold Post title:string body:text published:boolean --quiet 2>&1 | tail -1 || true

# 5. Install RSpec + create manual spec file
printf "  → rails generate rspec:install ...\n"
rails generate rspec:install --quiet 2>&1 | tail -1 || true

mkdir -p spec/models
cat > spec/models/post_spec.rb <<'SPEC'
require 'rails_helper'

RSpec.describe Post, type: :model do
  it "is valid with valid attributes" do
    post = Post.new(title: "Test", body: "Body", published: false)
    expect(post).to be_valid
  end
end
SPEC

# 6. Create + migrate database
printf "  → rails db:create && db:migrate ...\n"
rails db:create --quiet 2>&1 | tail -1 || true
rails db:migrate --quiet 2>&1 | tail -1 || true

# 7. Create a file with intentional RuboCop offenses
printf "  → creating rubocop_bait.rb with intentional offenses ...\n"
cat > app/models/rubocop_bait.rb <<'BAIT'
class RubocopBait < ApplicationRecord
  def messy_method()
    x = 1
    y =  2
    if x == 1
      puts     "hello world"
    end
    return   nil
  end
end
BAIT

# 8. Create a failing minitest test
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

# 9. Create a failing RSpec spec
printf "  → creating failing rspec spec ...\n"
cat > spec/models/post_fail_spec.rb <<'FAILSPEC'
require 'rails_helper'

RSpec.describe Post, type: :model do
  it "intentionally fails validation check" do
    post = Post.new(title: "Hello", body: "World", published: false)
    expect(post.title).to eq("Wrong Title On Purpose")
  end
end
FAILSPEC

printf "\n${BOLD}Setup complete. Running tests...${NC}\n"

# ══════════════════════════════════════════════════════
# Test sections
# ══════════════════════════════════════════════════════

# ── 1. rails generate ───────────────────────────────

section "Rails generate"

assert_output "rtk rails generate model Comment" \
    "files" \
    rtk rails generate model Comment post:references body:text

# Migrate the new model for later tests
rails db:migrate --quiet 2>&1 || true

# ── 2. rails db:migrate ─────────────────────────────

section "Rails db:migrate"

assert_output "rtk rails db:migrate (no-op)" \
    "db:migrate\|migrate\|already" \
    rtk rails db:migrate

# ── 3. rails db:migrate:status ──────────────────────

section "Rails db:migrate:status"

assert_output "rtk rails db:migrate:status" \
    "migration" \
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

# ── 7. rspec ────────────────────────────────────────

section "RSpec"

assert_output "rtk rspec (with failure)" \
    "failed" \
    rtk rspec

assert_output "rtk rspec spec/models/post_spec.rb (pass)" \
    "RSpec.*passed" \
    rtk rspec spec/models/post_spec.rb

assert_output "rtk rspec spec/models/post_fail_spec.rb (fail)" \
    "failed\|❌" \
    rtk rspec spec/models/post_fail_spec.rb

# ── 8. rubocop ──────────────────────────────────────

section "RuboCop"

assert_output "rtk rubocop (with offenses)" \
    "offense" \
    rtk rubocop

assert_output "rtk rubocop app/ (with offenses)" \
    "rubocop_bait\|offense" \
    rtk rubocop app/

# ── 9. bundle list ──────────────────────────────────

section "Bundle"

assert_output "rtk bundle list" \
    "gems\|Bundle" \
    rtk bundle list

assert_output "rtk bundle outdated" \
    "Bundle\|outdated\|up to date\|Gem\|Current" \
    rtk bundle outdated

assert_output "rtk bundle install (idempotent)" \
    "bundle install\|gems" \
    rtk bundle install

assert_output "rtk bundle update" \
    "gems\|bundle" \
    rtk bundle update

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
