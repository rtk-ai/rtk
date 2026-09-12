#!/usr/bin/env bash
# Sourced by changed-scope.sh and check-embed-scope.sh.
#
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
