#!/usr/bin/env python3
# rtk-hook-version: 3
# RTK Claude Code hook — rewrites commands to use rtk for token savings.
# Windows-compatible alternative to rtk-rewrite.sh. Requires only Python 3 and rtk >= 0.23.0.
# No jq dependency.
#
# This is a thin delegating hook: all rewrite logic lives in `rtk rewrite`,
# which is the single source of truth (src/discover/registry.rs).
# To add or change rewrite rules, edit the Rust registry — not this file.
#
# Exit code protocol for `rtk rewrite`:
#   0 + stdout  Rewrite found, no deny/ask rule matched → auto-allow
#   1           No RTK equivalent → pass through unchanged
#   2           Deny rule matched → pass through (Claude Code native deny handles it)
#   3 + stdout  Ask rule matched → rewrite but let Claude Code prompt the user

import json
import subprocess
import sys

_MIN_VERSION = (0, 23, 0)


def _check_rtk():
    """Return an error string if rtk is missing or too old, else None."""
    try:
        result = subprocess.run(["rtk", "--version"], capture_output=True, text=True)
        parts = result.stdout.strip().split()
        if len(parts) >= 2:
            try:
                version = tuple(int(x) for x in parts[1].split(".")[:3])
                if version < _MIN_VERSION:
                    return (
                        f"rtk {parts[1]} is too old (need >= 0.23.0). "
                        "Upgrade: cargo install rtk"
                    )
            except ValueError:
                pass
    except FileNotFoundError:
        return (
            "rtk is not installed or not in PATH. "
            "Install: https://github.com/rtk-ai/rtk#installation"
        )
    return None


def main():
    err = _check_rtk()
    if err:
        print(f"[rtk] WARNING: {err}", file=sys.stderr)
        sys.exit(0)

    try:
        data = json.load(sys.stdin)
    except Exception:
        sys.exit(0)

    cmd = data.get("tool_input", {}).get("command", "")
    if not cmd:
        sys.exit(0)

    # Delegate all rewrite + permission logic to the Rust binary.
    try:
        result = subprocess.run(
            ["rtk", "rewrite", cmd],
            capture_output=True,
            text=True,
        )
    except Exception:
        sys.exit(0)

    exit_code = result.returncode
    rewritten = result.stdout.strip()

    if exit_code == 0:
        # Rewrite found — if output is identical, command already uses RTK.
        if cmd == rewritten:
            sys.exit(0)
    elif exit_code == 1:
        # No RTK equivalent — pass through unchanged.
        sys.exit(0)
    elif exit_code == 2:
        # Deny rule matched — let Claude Code's native deny rule handle it.
        sys.exit(0)
    elif exit_code == 3:
        # Ask rule matched — rewrite but do NOT auto-allow so Claude Code prompts.
        pass
    else:
        sys.exit(0)

    updated_input = dict(data.get("tool_input", {}))
    updated_input["command"] = rewritten

    if exit_code == 3:
        # Ask: omit permissionDecision so Claude Code prompts the user.
        out = {
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "updatedInput": updated_input,
            }
        }
    else:
        # Allow: rewrite and auto-allow.
        out = {
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "permissionDecisionReason": "RTK auto-rewrite",
                "updatedInput": updated_input,
            }
        }

    print(json.dumps(out))


if __name__ == "__main__":
    main()
