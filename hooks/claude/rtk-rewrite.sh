#!/usr/bin/env bash
# rtk-hook-version: 4
# RTK Claude Code hook — rewrites commands to use rtk for token savings.
#
# All rewrite logic lives in `rtk hook claude-code` (pure Rust, no jq required).
# This script is a thin stdin pipe to the rtk binary.
#
# To add or change rewrite rules, edit the Rust registry — not this file.

if ! command -v rtk &>/dev/null; then
  echo "[rtk] WARNING: rtk is not installed or not in PATH. Hook cannot rewrite commands. Install: https://github.com/rtk-ai/rtk#installation" >&2
  exit 0
fi

exec rtk hook claude-code
