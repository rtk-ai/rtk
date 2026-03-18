#!/usr/bin/env bash
# rtk-hook-version: 3
# RTK Claude Code hook — rewrites commands to use rtk for token savings.
# v3: all JSON handling in Rust (no jq in primary path), pinned binary.
#
# This is a thin delegating hook: all rewrite logic lives in `rtk rewrite`,
# which is the single source of truth (src/discover/registry.rs).
# To add or change rewrite rules, edit the Rust registry — not this file.

# ── Resolve rtk binary (pinned path > PATH) ─────────────
# Priority: RTK_PATH env (testing/CI override) > pinned bin-path > PATH (with warning).
# RTK_PATH is intentional for CI and development; it requires the caller to
# explicitly set a specific env var, which is a narrower attack surface than PATH.
RTK_BIN="${RTK_PATH:-}"

if [ -z "$RTK_BIN" ]; then
  BIN_PATH_FILE="${XDG_CONFIG_HOME:-$HOME/.config}/rtk/bin-path"
  if [ -f "$BIN_PATH_FILE" ]; then
    RTK_BIN=$(cat "$BIN_PATH_FILE")
  fi
fi

if [ -z "$RTK_BIN" ] || [ ! -x "$RTK_BIN" ]; then
  RTK_BIN=$(command -v rtk 2>/dev/null || true)
  if [ -n "$RTK_BIN" ]; then
    echo "[rtk] WARNING: using PATH-resolved rtk ($RTK_BIN). Run \`rtk init -g\` to pin the binary path." >&2
  fi
fi

if [ -z "$RTK_BIN" ]; then
  echo "[rtk] WARNING: rtk not found. Install: https://github.com/rtk-ai/rtk#installation" >&2
  exit 0
fi

# ── Version guard ────────────────────────────────────────
RTK_VERSION=$("$RTK_BIN" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)
if [ -n "$RTK_VERSION" ]; then
  MAJOR=$(echo "$RTK_VERSION" | cut -d. -f1)
  MINOR=$(echo "$RTK_VERSION" | cut -d. -f2)
  # v3 hook requires >= 0.31.0 for --hook mode; fall back to v2 jq protocol
  if [ "$MAJOR" -eq 0 ] && [ "$MINOR" -lt 31 ]; then
    # ── v2 fallback (requires jq) ────────────────────────
    if ! command -v jq &>/dev/null; then
      echo "[rtk] WARNING: rtk $RTK_VERSION needs jq for hook. Upgrade rtk or install jq." >&2
      exit 0
    fi
    INPUT=$(cat)
    CMD=$(echo "$INPUT" | jq -r '.tool_input.command // empty')
    [ -z "$CMD" ] && exit 0
    REWRITTEN=$("$RTK_BIN" rewrite "$CMD" 2>/dev/null) || exit 0
    [ "$CMD" = "$REWRITTEN" ] && exit 0
    ORIGINAL_INPUT=$(echo "$INPUT" | jq -c '.tool_input')
    UPDATED_INPUT=$(echo "$ORIGINAL_INPUT" | jq --arg cmd "$REWRITTEN" '.command = $cmd')
    jq -n --argjson updated "$UPDATED_INPUT" \
      '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","permissionDecisionReason":"RTK auto-rewrite","updatedInput":$updated}}'
    exit 0
  fi
fi

# ── Primary path: rtk handles all JSON (no jq needed) ───
exec cat | "$RTK_BIN" rewrite --hook
