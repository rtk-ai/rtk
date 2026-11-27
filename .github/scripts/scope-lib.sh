#!/usr/bin/env bash
# Sourced by changed-scope.sh and check-embed-scope.sh.

# load_pathspecs <conf> populates the PATHSPECS array with the git pathspecs in
# <conf>, dropping comments and blanks. Returns non-zero if the file is
# unreadable or lists nothing, so callers can fall back to building.

load_pathspecs() {
    local conf="$1" line
    PATHSPECS=()

    [ -r "$conf" ] || return 1

    # `|| [ -n "$line" ]` so a final line with no trailing newline still counts.
    while IFS= read -r line || [ -n "$line" ]; do
        case "$line" in ''|'#'*) continue ;; esac
        PATHSPECS+=("$line")
    done < "$conf"

    [ "${#PATHSPECS[@]}" -gt 0 ]
}

# enter_fixture_repo creates an empty repository in a temp directory, names it
# FIXTURE_REPO and enters it. Callers must stop on failure (and so on this
# function being missing), or their fixture commands run in the caller's own
# repository.
enter_fixture_repo() {
    # `git -C ""` is a documented no-op that runs against the caller's own
    # repository, so an unchecked mktemp here would rewrite a contributor's git
    # config and commit to their branch. Verify the directory, then cd into it
    # and use plain git, so no command can be aimed anywhere else.
    if ! FIXTURE_REPO="$(mktemp -d)" || [ ! -d "$FIXTURE_REPO" ]; then
        echo "FAIL: could not create fixture repo"
        return 1
    fi
    cd "$FIXTURE_REPO" || { echo "FAIL: could not enter fixture repo"; return 1; }

    # The fixture must not inherit the contributor's config: commit.gpgsign or
    # a global core.hooksPath would fail its commits, and every `assert skip`
    # would then misreport as build.
    export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null
    # Nor its repository: run from a git hook, or with GIT_DIR set, these
    # commands would otherwise commit to that one rather than to the fixture.
    # shellcheck disable=SC2046
    unset $(git rev-parse --local-env-vars)
    git init -q .
}
