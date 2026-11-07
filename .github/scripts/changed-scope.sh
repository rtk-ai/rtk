#!/usr/bin/env bash
set -uo pipefail

# changed-scope.sh — decide whether a diff range warrants a build.
#
# Usage:
#   changed-scope.sh <git-diff-range-args...>
#     changed-scope.sh "$BEFORE" "$AFTER"
#     changed-scope.sh "origin/develop...HEAD"
#   changed-scope.sh --self-test
#
# Exit status:
#   3  the range touches nothing outside .github/inert-paths.conf
#   0  build required
#   *  anything else is an error, and callers must build
#
# 3, not 1, because bash hands out the low statuses on its own: 1 for a set -u
# violation or a failed command, 2 for a syntax error, 127 for a missing one.
# Reserving an improbable status means an accident can never be read as a skip.
#
# --no-renames throughout: rename detection reports only a rename's
# destination, so moving an embedded file into docs/ would otherwise look inert
# on a commit that no longer compiles.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=scope-lib.sh
. "$SCRIPT_DIR/scope-lib.sh"

DEFAULT_CONF="$SCRIPT_DIR/../inert-paths.conf"

BUILD=0
NO_BUILD=3

classify() {   # classify <conf> <git-diff-range-args...>
    local conf="$1"; shift
    if ! load_pathspecs "$conf"; then
        echo "changed-scope: $conf is unreadable or lists no pathspecs" >&2
        return "$BUILD"
    fi

    local all kept
    # `:/` plus the conf's `,top` anchor this at the repo root. Pathspecs are
    # otherwise relative to the working directory, and from a directory the
    # change did not touch, everything reads as inert.
    all="$(git diff --no-renames --name-only "$@" -- ':/')" || return "$BUILD"
    kept="$(git diff --no-renames --name-only "$@" -- ':/' "${PATHSPECS[@]}")" || return "$BUILD"

    if [ -z "$all" ]; then
        echo "changed-scope: range changed nothing" >&2
        return "$BUILD"
    fi

    if [ -z "$kept" ]; then
        echo "changed-scope: all $(printf '%s\n' "$all" | wc -l) changed file(s) inert" >&2
        return "$NO_BUILD"
    fi

    echo "changed-scope: $(printf '%s\n' "$kept" | wc -l) changed file(s) require a build:" >&2
    printf '%s\n' "$kept" | sed 's/^/  /' >&2
    return "$BUILD"
}

self_test() {
    local conf="$DEFAULT_CONF" failed=0

    enter_fixture_repo || exit 1
    git config user.email t@t
    git config user.name t
    git commit -q --allow-empty -m base

    # Each probe commits its files and diffs only that commit, so files left
    # behind by earlier probes cannot leak into a later verdict.
    assert() {   # assert build|skip <file>...
        local want="$1"; shift
        local f
        for f in "$@"; do
            mkdir -p "$(dirname "$f")"
            echo change >> "$f"
        done
        # -f because GIT_CONFIG_GLOBAL does not reach the default global ignore
        # file. An ignored probe would commit nothing and be judged on the diff
        # of the probe before it.
        git add -Af
        git commit -qm probe || { echo "FAIL: could not commit probe: $*"; failed=1; return; }

        classify "$conf" HEAD~1 HEAD >/dev/null 2>&1
        local rc=$? got
        case "$rc" in
            "$BUILD") got=build ;;
            "$NO_BUILD") got=skip ;;
            *) got="rc=$rc" ;;
        esac
        if [ "$got" != "$want" ]; then
            echo "FAIL: want $want, got $got, for: $*"
            failed=1
        fi
    }

    # The trap this exists to prevent: hooks/*.md are include_str! inputs, and
    # a pathspec without `,glob` would exclude them.
    assert build hooks/claude/rtk-awareness.md
    assert build hooks/README.md
    assert build src/filters/git.toml
    assert build src/filters/README.md
    assert build src/main.rs
    assert build build.rs
    assert build Cargo.toml
    assert build Cargo.lock
    assert build tests/fixtures/f.txt
    assert build scripts/benchmark.sh
    assert build .github/workflows/ci.yml
    assert build .github/inert-paths.conf
    assert build .github/scripts/changed-scope.sh
    assert build install.sh
    assert build openclaw/index.ts
    assert build .semgrep.yml
    assert build some/brand/new/thing.txt

    assert skip README.md
    assert skip README_fr.md
    assert skip CLAUDE.md
    assert skip CONTRIBUTING.md
    assert build LICENSE
    assert skip docs/guide/installation.md
    assert skip docs/guide/getting-started/installation.md
    # Tests embed these pages to check their examples.
    assert build docs/guide/pipelines/methods.md
    assert build docs/guide/proxy/compression.md
    assert build docs/guide/resources/agent-coverage.md
    assert build docs/guide/brand-new-section/page.md
    assert skip docs/usage/TRACKING.md
    assert skip .claude/rules/cli-testing.md
    assert skip .claude/skills/ship/SKILL.md
    # tests/hook_decision_protocol_test.rs runs this one.
    assert build .claude/hooks/rtk-rewrite.sh
    assert skip .github/PULL_REQUEST_TEMPLATE.md
    # `*.md` would stop at .github/'s direct children and miss this one.
    assert skip .github/workflows/CICD.md
    assert skip Formula/rtk.rb

    # Real merged docs-only PRs: #3110, #3103.
    assert skip docs/guide/analytics/gain.md docs/usage/TRACKING.md
    assert skip .claude/rules/cli-testing.md CLAUDE.md

    # One build input among docs still builds.
    assert build README.md src/main.rs
    assert build docs/usage/x.md hooks/cline/rules.md

    # A degenerate config must never read as a skip.
    conf=/dev/null      assert build README.md
    conf=/nonexistent   assert build README.md

    cd / || true
    rm -rf "$FIXTURE_REPO"

    if [ "$failed" -ne 0 ]; then
        echo "FAIL: changed-scope self-test"
        exit 1
    fi
    echo "PASS: changed-scope self-test"
}

if [ "${1:-}" = "--self-test" ]; then
    self_test
    exit 0
fi

classify "$DEFAULT_CONF" "$@"
