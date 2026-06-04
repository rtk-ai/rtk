"""Hermes plugin adapter for RTK command rewriting and output filtering.

All rewrite logic lives in RTK's Rust ``rtk rewrite`` command; this module
also provides a pure-Python ``transform_tool_result`` filter for high-token
tools (browser_navigate, execute_code, read_file).
"""

import json
import re
import shutil
import subprocess
import sys


ACCEPTED_REWRITE_RETURN_CODES = {0, 3}
EXPECTED_PASSTHROUGH_RETURN_CODES = {1, 2}
_rtk_available = None
_rtk_missing_warned = False

# Accessibility-tree noise patterns produced by browser_navigate snapshots.
_LAYOUT_NOISE = re.compile(
    r"^\s*-?\s*Layout(?:Table(?:Row|Cell|Section)?|Inline[A-Za-z]*|Block[A-Za-z]*)"
    r"(?:\s+\"[^\"]{0,60}\"\s*)?$"
)

_SNAPSHOT_MAX_LINES = 120
_TEXT_MAX_LINES     = 200


def register(ctx):
    """Register the Hermes pre-tool and transform-result callbacks."""
    if not _check_rtk():
        return

    ctx.register_hook("pre_tool_call", _pre_tool_call)
    ctx.register_hook("transform_tool_result", _transform_tool_result)


def _check_rtk():
    """Return whether the rtk binary is in PATH, warning once when missing."""
    global _rtk_available, _rtk_missing_warned

    if _rtk_available is None:
        _rtk_available = shutil.which("rtk") is not None

    if not _rtk_available and not _rtk_missing_warned:
        _warn("rtk binary not found in PATH; Hermes hook not registered")
        _rtk_missing_warned = True

    return _rtk_available


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
                ["rtk", "rewrite", command],
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


def _transform_tool_result(tool_name=None, result=None, **_kwargs):
    """Filter high-token tool outputs before they enter the conversation context."""
    if tool_name == "browser_navigate":
        return _filter_browser(result)
    if tool_name in ("execute_code", "read_file"):
        return _filter_text(result)


def _filter_browser(result):
    """Strip Layout* accessibility noise from browser_navigate JSON and truncate snapshot."""
    if not isinstance(result, str) or not result.strip():
        return
    before = len(result)
    try:
        data = json.loads(result)
    except (json.JSONDecodeError, ValueError):
        return _filter_text(result)

    snapshot = data.get("snapshot", "")
    if snapshot:
        lines = snapshot.splitlines()
        lines = [l for l in lines if l.strip() and not _LAYOUT_NOISE.match(l)]
        seen, deduped = set(), []
        for l in lines:
            key = l.strip()
            if key not in seen:
                seen.add(key)
                deduped.append(l)
        if len(deduped) > _SNAPSHOT_MAX_LINES:
            deduped = deduped[:_SNAPSHOT_MAX_LINES] + [f"... [{len(lines) - _SNAPSHOT_MAX_LINES} lines truncated]"]
        data["snapshot"] = "\n".join(deduped)

    # Drop noisy metadata fields
    for key in ("stealth_warning", "stealth_features"):
        data.pop(key, None)

    filtered = json.dumps(data, ensure_ascii=False)
    after = len(filtered)
    if after < before:
        print(
            f"rtk: filtered browser_navigate: {before} -> {after} chars"
            f" ({100*(before-after)//before}% reduction)",
            file=sys.stderr,
        )
        return filtered


def _filter_text(result):
    """Strip blank lines, deduplicate, and truncate generic text outputs."""
    if not isinstance(result, str) or not result.strip():
        return
    before = len(result)
    lines = result.splitlines()
    seen, deduped = set(), []
    for l in lines:
        key = l.strip()
        if not key:
            continue
        if key not in seen:
            seen.add(key)
            deduped.append(l)
    if len(deduped) > _TEXT_MAX_LINES:
        deduped = deduped[:_TEXT_MAX_LINES] + [f"... [{len(lines) - _TEXT_MAX_LINES} lines truncated]"]
    filtered = "\n".join(deduped)
    after = len(filtered)
    if after < before:
        print(
            f"rtk: filtered text ({after}/{before} chars, {100*(before-after)//before}% reduction)",
            file=sys.stderr,
        )
        return filtered


def _warn(message):
    print(f"rtk: hermes plugin warning: {message}", file=sys.stderr)
