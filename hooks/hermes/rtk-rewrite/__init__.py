"""Hermes plugin adapter for RTK command rewriting.

All rewrite logic lives in RTK's Rust ``rtk rewrite`` command; this module
only bridges Hermes ``pre_tool_call`` payloads to that command and fails open.
"""

import os
import shutil
import subprocess
import sys


ACCEPTED_REWRITE_RETURN_CODES = {0, 3}
EXPECTED_PASSTHROUGH_RETURN_CODES = {1, 2}
_rtk_bin = None
_rtk_missing_warned = False


def register(ctx):
    """Register the Hermes pre-tool callback."""
    if not _check_rtk():
        return

    ctx.register_hook("pre_tool_call", _pre_tool_call)


def _check_rtk():
    """Return whether the rtk binary is available.

    Checks common install locations first (absolute paths), then falls
    back to PATH lookup.  This is essential on Windows and in
    service/daemon-managed gateways where the agent process may not
    inherit the user's shell PATH.
    """
    global _rtk_bin, _rtk_missing_warned

    if _rtk_bin is None:
        candidates = [
            os.path.join(os.path.expanduser("~"), ".local", "bin", "rtk"),
            os.path.join(os.path.expanduser("~"), ".local", "bin", "rtk.exe"),
            "/usr/local/bin/rtk",
        ]
        for path in candidates:
            if os.path.isfile(path) and os.access(path, os.X_OK):
                _rtk_bin = path
                break

        if _rtk_bin is None:
            found = shutil.which("rtk")
            if found:
                _rtk_bin = found

    if not _rtk_bin and not _rtk_missing_warned:
        _warn("rtk binary not found; Hermes hook not registered")
        _rtk_missing_warned = True

    return _rtk_bin is not None


def _pre_tool_call(tool_name=None, args=None, **_kwargs):
    """Rewrite mutable Hermes terminal command args when RTK provides a change."""
    try:
        if tool_name != "terminal" or not isinstance(args, dict):
            return

        command = args.get("command")
        if not isinstance(command, str) or not command.strip():
            return

        try:
            result = subprocess.run(
                [_rtk_bin, "rewrite", command],
                shell=False,
                timeout=2,
                capture_output=True,
                text=True,
            )
        except subprocess.TimeoutExpired:
            _warn("rtk rewrite timed out")
            return

        if result.returncode not in ACCEPTED_REWRITE_RETURN_CODES:
            if result.returncode not in EXPECTED_PASSTHROUGH_RETURN_CODES:
                details = f"rtk rewrite failed with exit {result.returncode}"
                stderr = result.stderr.strip()
                if stderr:
                    details = f"{details}: {stderr}"
                _warn(details)
            return

        rewritten = result.stdout.strip()
        if rewritten and rewritten != command:
            args["command"] = rewritten
    except Exception as e:
        _warn(str(e))
        return


def _warn(message):
    print(f"rtk: hermes plugin warning: {message}", file=sys.stderr)
