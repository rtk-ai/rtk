#!/usr/bin/env bash
#
# RTK Smoke Tests — Ruby (RSpec, RuboCop, Minitest, Bundle)
# Creates a minimal Rails app, exercises all Ruby RTK filters, then cleans up.
# Usage: bash scripts/test-ruby.sh
#
# Prerequisites: rtk (installed), ruby, bundler, rails gem
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

printf "${BOLD}RTK Smoke Tests — Ruby (RSpec, RuboCop, Minitest, Bundle)${NC}\n"
printf "Binary: %s (%s)\n" "$RTK" "$(rtk --version)"
printf "Ruby: %s\n" "$(ruby --version)"
printf "Rails: %s\n" "$(rails --version)"
printf "Bundler: %s\n" "$(bundle --version)"
printf "Date: %s\n\n" "$(date '+%Y-%m-%d %H:%M')"

# ── Temp dir + cleanup trap ──────────────────────────

TMPDIR=$(mktemp -d /tmp/rtk-ruby-smoke-XXXXXX)
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

# 4. Generate scaffold (creates model + minitest files)
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

# 8. Create a failing RSpec spec
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

# 9. Create an RSpec spec with pending example
printf "  → creating rspec spec with pending example ...\n"
cat > spec/models/post_pending_spec.rb <<'PENDSPEC'
require 'rails_helper'

RSpec.describe Post, type: :model do
  it "is valid with title" do
    post = Post.new(title: "OK", body: "Body", published: false)
    expect(post).to be_valid
  end

  it "will support markdown later" do
    pending "Not yet implemented"
    expect(Post.new.render_markdown).to eq("<p>hello</p>")
  end
end
PENDSPEC

# 10. Create a failing minitest test
printf "  → creating failing minitest test ...\n"
cat > test/models/post_fail_test.rb <<'FAILTEST'
require "test_helper"

class PostFailTest < ActiveSupport::TestCase
  test "intentionally fails" do
    assert_equal "wrong", Post.new(title: "right").title
  end
end
FAILTEST

# 11. Create a passing minitest test
printf "  → creating passing minitest test ...\n"
cat > test/models/post_pass_test.rb <<'PASSTEST'
require "test_helper"

class PostPassTest < ActiveSupport::TestCase
  test "post is valid" do
    post = Post.new(title: "OK", body: "Body", published: false)
    assert post.valid?
  end
end
PASSTEST

printf "\n${BOLD}Setup complete. Running tests...${NC}\n"

# ══════════════════════════════════════════════════════
# Test sections
# ══════════════════════════════════════════════════════

# ── 1. RSpec ─────────────────────────────────────────

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

# ── 2. RuboCop ───────────────────────────────────────

section "RuboCop"

assert_output "rtk rubocop (with offenses)" \
    "offense" \
    rtk rubocop

assert_output "rtk rubocop app/ (with offenses)" \
    "rubocop_bait\|offense" \
    rtk rubocop app/

# ── 3. Minitest (rake test) ──────────────────────────

section "Minitest (rake test)"

assert_output "rtk rake test (with failure)" \
    "failure\|error\|FAIL" \
    rtk rake test

assert_output "rtk rake test single passing file" \
    "ok rake test\|0 failures" \
    rtk rake test TEST=test/models/post_pass_test.rb

assert_exit_nonzero "rtk rake test single failing file" \
    "failure\|FAIL" \
    rtk rake test test/models/post_fail_test.rb

# ── 4. Bundle install ────────────────────────────────

section "Bundle install"

assert_output "rtk bundle install (idempotent)" \
    "bundle\|ok\|complete\|install" \
    rtk bundle install

# ── 5. Exit code preservation ────────────────────────

section "Exit code preservation"

assert_exit_nonzero "rtk rspec exits non-zero on failure" \
    "failed\|failure" \
    rtk rspec spec/models/post_fail_spec.rb

assert_exit_nonzero "rtk rubocop exits non-zero on offenses" \
    "offense" \
    rtk rubocop app/models/rubocop_bait.rb

assert_exit_nonzero "rtk rake test exits non-zero on failure" \
    "failure\|FAIL" \
    rtk rake test test/models/post_fail_test.rb

# ── 6. bundle exec variants ─────────────────────────

section "bundle exec variants"

assert_output "bundle exec rspec spec/models/post_spec.rb" \
    "passed\|example" \
    rtk bundle exec rspec spec/models/post_spec.rb

assert_output "bundle exec rubocop app/" \
    "offense" \
    rtk bundle exec rubocop app/

# ── 7. RuboCop autocorrect ───────────────────────────

section "RuboCop autocorrect"

# Copy bait file so autocorrect has something to fix
cp app/models/rubocop_bait.rb app/models/rubocop_bait_ac.rb
sed -i.bak 's/RubocopBait/RubocopBaitAc/' app/models/rubocop_bait_ac.rb

assert_output "rtk rubocop -A (autocorrect)" \
    "autocorrected\|rubocop\|ok\|offense\|inspected" \
    rtk rubocop -A app/models/rubocop_bait_ac.rb

# Clean up autocorrect test file
rm -f app/models/rubocop_bait_ac.rb app/models/rubocop_bait_ac.rb.bak

# ── 8. RSpec pending ─────────────────────────────────

section "RSpec pending"

assert_output "rtk rspec with pending example" \
    "pending" \
    rtk rspec spec/models/post_pending_spec.rb

# ── 9. RSpec text fallback ───────────────────────────

section "RSpec text fallback"

assert_output "rtk rspec --format documentation (text path)" \
    "valid\|example\|post" \
    rtk rspec --format documentation spec/models/post_spec.rb

# ── 10. RSpec empty suite ────────────────────────────

section "RSpec empty suite"

assert_output "rtk rspec nonexistent tag" \
    "0 examples\|No examples" \
    rtk rspec --tag nonexistent spec/models/post_spec.rb

# ── 11. Token savings ────────────────────────────────

section "Token savings"

# rspec (passing spec)
raw_len=$( (bundle exec rspec spec/models/post_spec.rb 2>&1 || true) | wc -c | tr -d ' ')
rtk_len=$( (rtk rspec spec/models/post_spec.rb 2>&1 || true) | wc -c | tr -d ' ')
if [[ "$rtk_len" -lt "$raw_len" ]]; then
    PASS=$((PASS + 1))
    printf "  ${GREEN}PASS${NC}  rspec: rtk (%s bytes) < raw (%s bytes)\n" "$rtk_len" "$raw_len"
else
    FAIL=$((FAIL + 1))
    FAILURES+=("token savings: rspec")
    printf "  ${RED}FAIL${NC}  rspec: rtk (%s bytes) >= raw (%s bytes)\n" "$rtk_len" "$raw_len"
fi

# rubocop (exits non-zero on offenses, so || true)
raw_len=$( (bundle exec rubocop app/ 2>&1 || true) | wc -c | tr -d ' ')
rtk_len=$( (rtk rubocop app/ 2>&1 || true) | wc -c | tr -d ' ')
if [[ "$rtk_len" -lt "$raw_len" ]]; then
    PASS=$((PASS + 1))
    printf "  ${GREEN}PASS${NC}  rubocop: rtk (%s bytes) < raw (%s bytes)\n" "$rtk_len" "$raw_len"
else
    FAIL=$((FAIL + 1))
    FAILURES+=("token savings: rubocop")
    printf "  ${RED}FAIL${NC}  rubocop: rtk (%s bytes) >= raw (%s bytes)\n" "$rtk_len" "$raw_len"
fi

# rake test (passing file)
raw_len=$( (bundle exec rake test TEST=test/models/post_pass_test.rb 2>&1 || true) | wc -c | tr -d ' ')
rtk_len=$( (rtk rake test test/models/post_pass_test.rb 2>&1 || true) | wc -c | tr -d ' ')
if [[ "$rtk_len" -lt "$raw_len" ]]; then
    PASS=$((PASS + 1))
    printf "  ${GREEN}PASS${NC}  rake test: rtk (%s bytes) < raw (%s bytes)\n" "$rtk_len" "$raw_len"
else
    FAIL=$((FAIL + 1))
    FAILURES+=("token savings: rake test")
    printf "  ${RED}FAIL${NC}  rake test: rtk (%s bytes) >= raw (%s bytes)\n" "$rtk_len" "$raw_len"
fi

# bundle install (idempotent)
raw_len=$( (bundle install 2>&1 || true) | wc -c | tr -d ' ')
rtk_len=$( (rtk bundle install 2>&1 || true) | wc -c | tr -d ' ')
if [[ "$rtk_len" -lt "$raw_len" ]]; then
    PASS=$((PASS + 1))
    printf "  ${GREEN}PASS${NC}  bundle install: rtk (%s bytes) < raw (%s bytes)\n" "$rtk_len" "$raw_len"
else
    FAIL=$((FAIL + 1))
    FAILURES+=("token savings: bundle install")
    printf "  ${RED}FAIL${NC}  bundle install: rtk (%s bytes) >= raw (%s bytes)\n" "$rtk_len" "$raw_len"
fi

# ── 12. Verbose flag ─────────────────────────────────

section "Verbose flag (-v)"

assert_output "rtk -v rspec (verbose)" \
    "RSpec\|passed\|Running\|example" \
    rtk -v rspec spec/models/post_spec.rb

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
