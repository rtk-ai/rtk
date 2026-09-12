#!/usr/bin/env bash
set -uo pipefail

run_self_test() {
    set -e

    local script_path
    local temp_dir
    local status
    script_path="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
    temp_dir=$(mktemp -d)
    trap 'rm -f "$temp_dir/success.log" "$temp_dir/failure.log"; rmdir "$temp_dir"' RETURN

    "$script_path" "$temp_dir/success.log" bash -c 'echo stdout; echo stderr >&2'
    grep -qx 'stdout' "$temp_dir/success.log"
    grep -qx 'stderr' "$temp_dir/success.log"

    set +e
    "$script_path" "$temp_dir/failure.log" bash -c 'echo failed; exit 23'
    status=$?
    set -e

    if [ "$status" -ne 23 ]; then
        echo "FAIL: expected command exit code 23, got $status"
        return 1
    fi
    grep -qx 'failed' "$temp_dir/failure.log"

    echo "PASS: command output is logged and its exit code is preserved"
}

if [ "${1:-}" = "--self-test" ]; then
    run_self_test
    exit $?
fi

if [ "$#" -lt 2 ]; then
    echo "Usage: $0 <log-file> <command> [args...]" >&2
    exit 2
fi

log_file=$1
shift

"$@" 2>&1 | tee "$log_file"
