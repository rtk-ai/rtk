#!/usr/bin/env python3
"""Swival command_middleware adapter for RTK.

Swival invokes this script for every command it is about to run, passing a
JSON request on stdin and reading a JSON response from stdout. All rewrite
logic lives in RTK's ``rtk rewrite`` command; this adapter only translates
between Swival's middleware protocol and that command, and it always fails
open so a misbehaving adapter can never block the user's command.

Request (stdin):
    {"phase": "before", "mode": "shell", "command": "git status"}
    {"phase": "before", "mode": "argv",  "command": ["git", "status"]}

Response (stdout):
    {"action": "allow", "mode": "shell", "command": "rtk git status"}   # rewritten
    {"action": "allow"}                                                  # unchanged
"""

import json
import shlex
import shutil
import subprocess
import sys

# rtk rewrite emits the rewritten command on success (0) and on the
# "rewrite applied" status (3). 1/2 mean "no rewrite / passthrough".
ACCEPTED_REWRITE_RETURN_CODES = {0, 3}
EXPECTED_PASSTHROUGH_RETURN_CODES = {1, 2}


def main():
    request = _read_request()
    if request is None:
        return _allow()

    mode = request.get("mode", "shell")
    raw = request.get("command")
    command = _to_command_string(raw, mode)
    if not command or not command.strip():
        return _allow()

    rewritten = _rewrite(command)
    if rewritten is None or rewritten == command:
        return _allow()

    return _allow(mode=mode, command=_to_command_value(rewritten, mode))


def _read_request():
    try:
        data = sys.stdin.read()
    except Exception as e:
        _warn(f"failed to read stdin: {e}")
        return None

    if not data.strip():
        return None

    try:
        request = json.loads(data)
    except json.JSONDecodeError as e:
        _warn(f"invalid JSON request: {e}")
        return None

    return request if isinstance(request, dict) else None


def _to_command_string(raw, mode):
    """Normalize Swival's command payload to a single shell string."""
    if mode == "argv" and isinstance(raw, list):
        return shlex.join(str(part) for part in raw)
    if isinstance(raw, str):
        return raw
    return None


def _to_command_value(command, mode):
    """Return the rewritten command in the same shape Swival sent it."""
    if mode == "argv":
        return shlex.split(command)
    return command


def _rewrite(command):
    if shutil.which("rtk") is None:
        _warn("rtk binary not found in PATH")
        return None

    try:
        result = subprocess.run(
            ["rtk", "rewrite", command],
            shell=False,
            timeout=2,
            capture_output=True,
            text=True,
        )
    except subprocess.TimeoutExpired:
        _warn("rtk rewrite timed out")
        return None
    except Exception as e:
        _warn(str(e))
        return None

    if result.returncode not in ACCEPTED_REWRITE_RETURN_CODES:
        if result.returncode not in EXPECTED_PASSTHROUGH_RETURN_CODES:
            details = f"rtk rewrite failed with exit {result.returncode}"
            stderr = result.stderr.strip()
            if stderr:
                details = f"{details}: {stderr}"
            _warn(details)
        return None

    rewritten = result.stdout.strip()
    return rewritten or None


def _allow(mode=None, command=None):
    response = {"action": "allow"}
    if command is not None:
        response["mode"] = mode
        response["command"] = command
    _emit(response)


def _emit(response):
    try:
        sys.stdout.write(json.dumps(response))
    except Exception as e:
        _warn(f"failed to write response: {e}")


def _warn(message):
    print(f"rtk: swival adapter warning: {message}", file=sys.stderr)


if __name__ == "__main__":
    main()
