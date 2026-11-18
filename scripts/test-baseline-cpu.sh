#!/usr/bin/env bash
# Exercise the shipped executable, not a scan for SIMD instructions: memchr and
# aho-corasick legitimately contain AVX2 kernels behind runtime feature checks.
set -euo pipefail
ulimit -c 0

if [[ $# != 1 ]]; then
  echo "Usage: $0 /path/to/rtk-or-rtk-proxy" >&2
  exit 2
fi
binary=$(realpath "$1")
qemu=${QEMU_X86_64:-qemu-x86_64}
command -v "$qemu" >/dev/null
[[ -x "$binary" ]]
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# Do not read or modify the runner's RTK configuration or send telemetry.
export HOME="$work/home" XDG_CONFIG_HOME="$work/config" XDG_DATA_HOME="$work/data"
export RTK_TELEMETRY_DISABLED=1
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME"
printf 'baseline CPU smoke test\n' > "$work/input.txt"
cd "$work"

# qemu64 has no AVX; IvyBridge has AVX but no AVX2. Both regressed in 1.2.0.
for cpu in qemu64 IvyBridge; do
  echo "Checking $(basename "$binary") on $cpu"
  timeout 60s "$qemu" -cpu "$cpu" "$binary" --version
  timeout 60s "$qemu" -cpu "$cpu" "$binary" --help > /dev/null
  if [[ $(basename "$binary") != rtk-proxy ]]; then
    # Exercise allocation and the bundled SQLite path beyond early startup.
    timeout 60s "$qemu" -cpu "$cpu" "$binary" read "$work/input.txt" > "$work/output.txt"
    cmp "$work/input.txt" "$work/output.txt"
    timeout 60s "$qemu" -cpu "$cpu" "$binary" gain > /dev/null
  fi
done
