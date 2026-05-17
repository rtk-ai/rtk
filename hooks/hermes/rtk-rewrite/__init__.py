"""Hermes plugin adapter for RTK command rewriting with savings display.

All rewrite logic lives in RTK's Rust ``rtk rewrite`` command; this module
only bridges Hermes ``pre_tool_call`` payloads to that command and fails open.

On ``transform_tool_result`` for ALL tools, it reads RTK's tracking DB
and appends a one-line savings summary to the tool result. It also
compresses large non-terminal tool results to save additional tokens.
"""

import json
import os
import re
import shutil
import sqlite3
import subprocess
import sys
from pathlib import Path


ACCEPTED_REWRITE_RETURN_CODES = {0, 3}
EXPECTED_PASSTHROUGH_RETURN_CODES = {1, 2}
_rtk_available = None
_rtk_missing_warned = False

# Tools whose results are NOT compressed (terminal is handled by rtk rewrite,
# and some tools return small or critical results we shouldn't mangle).
_SKIP_COMPRESS_TOOLS = {
    "terminal",        # handled by rtk rewrite
    "patch",           # diff results must stay intact for LLM to verify
    "todo",            # small results
    "clarify",         # user-facing, must be pristine
    "write_file",      # small confirmation results
    "memory",          # small critical results
    "skill_manage",    # small results
    "mcp_agentmemory_memory_save",  # small
    "mcp_agentmemory_memory_recall", # small
    "mcp_agentmemory_memory_sessions", # small
    "mcp_agentmemory_memory_smart_search", # small
}

# Minimum result size (chars) before compression kicks in.
_COMPRESS_THRESHOLD = 2000

# Maximum length for any single string value in JSON output.
_MAX_VALUE_LEN = 500

# Tracking DB for non-terminal compression savings.
_COMPRESSION_DB_PATH = Path.home() / ".local" / "share" / "rtk" / "hermes_compression.db"


def _rtk_db_path():
    """Resolve the RTK tracking database path."""
    xdg = os.environ.get("XDG_DATA_HOME", "")
    if xdg:
        base = Path(xdg)
    else:
        base = Path.home() / ".local" / "share"
    return base / "rtk" / "history.db"


def _read_rtk_savings():
    """Read cumulative savings from the RTK tracking database.

    Returns (total_saved_tokens, total_commands, avg_pct) or (0, 0, 0.0)
    if the DB is missing / unreadable.
    """
    db_path = _rtk_db_path()
    if not db_path.exists():
        return 0, 0, 0.0
    try:
        conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True, timeout=1)
        conn.row_factory = sqlite3.Row
        cur = conn.execute(
            "SELECT COALESCE(SUM(saved_tokens),0) AS saved,"
            " COUNT(*) AS cnt,"
            " COALESCE(ROUND(AVG(savings_pct),1),0.0) AS avg_pct"
            " FROM commands"
        )
        row = cur.fetchone()
        conn.close()
        return row["saved"], row["cnt"], row["avg_pct"]
    except Exception as e:
        _warn(f"rtk savings read error: {e}")
        return 0, 0, 0.0


def _ensure_compression_db():
    """Create the compression tracking DB if it doesn't exist."""
    _COMPRESSION_DB_PATH.parent.mkdir(parents=True, exist_ok=True)
    if not _COMPRESSION_DB_PATH.exists():
        conn = sqlite3.connect(str(_COMPRESSION_DB_PATH))
        conn.execute(
            "CREATE TABLE IF NOT EXISTS compression_stats ("
            " id INTEGER PRIMARY KEY AUTOINCREMENT,"
            " tool_name TEXT NOT NULL,"
            " original_chars INTEGER NOT NULL,"
            " compressed_chars INTEGER NOT NULL,"
            " saved_chars INTEGER NOT NULL,"
            " saved_pct REAL NOT NULL,"
            " timestamp TEXT DEFAULT CURRENT_TIMESTAMP"
            ")"
        )
        conn.commit()
        conn.close()


def _record_compression(tool_name, original_len, compressed_len):
    """Record a compression event in the tracking DB."""
    if original_len <= 0 or compressed_len >= original_len:
        return  # no actual savings or invalid input, skip
    saved_chars = original_len - compressed_len
    saved_pct = round(saved_chars / original_len * 100, 1)
    try:
        _ensure_compression_db()
        conn = sqlite3.connect(str(_COMPRESSION_DB_PATH), timeout=1)
        conn.execute(
            "INSERT INTO compression_stats (tool_name, original_chars, compressed_chars, saved_chars, saved_pct)"
            " VALUES (?, ?, ?, ?, ?)",
            (tool_name, original_len, compressed_len, saved_chars, saved_pct),
        )
        conn.commit()
        conn.close()
    except Exception as e:
        _warn(f"compression record error: {e}")


def _read_compression_savings():
    """Read cumulative compression savings from our tracking DB.

    Returns (total_saved_chars, total_compressions, avg_pct) or (0, 0, 0.0).
    """
    if not _COMPRESSION_DB_PATH.exists():
        return 0, 0, 0.0
    try:
        conn = sqlite3.connect(f"file:{_COMPRESSION_DB_PATH}?mode=ro", uri=True, timeout=1)
        cur = conn.execute(
            "SELECT COALESCE(SUM(saved_chars),0),"
            " COUNT(*),"
            " COALESCE(ROUND(AVG(saved_pct),1),0.0)"
            " FROM compression_stats"
        )
        row = cur.fetchone()
        conn.close()
        return row[0], row[1], row[2]
    except Exception as e:
        _warn(f"compression savings read error: {e}")
        return 0, 0, 0.0


def _format_tokens(n):
    """Human-friendly token count."""
    if n >= 1_000_000:
        return f"{n/1_000_000:.1f}M"
    if n >= 1_000:
        return f"{n/1_000:.1f}K"
    return str(n)


def _format_chars(n):
    """Human-friendly char count (approximated as ~4 chars/token)."""
    tokens = n / 4
    return _format_tokens(int(tokens))


def _truncate_long_values(obj, max_len=_MAX_VALUE_LEN):
    """Recursively truncate long string values in a JSON structure.

    Keeps structure intact but shortens overly long string values.
    """
    if isinstance(obj, dict):
        return {k: _truncate_long_values(v, max_len) for k, v in obj.items()}
    elif isinstance(obj, list):
        return [_truncate_long_values(item, max_len) for item in obj]
    elif isinstance(obj, str):
        if len(obj) > max_len:
            half = max_len // 2 - 10
            return obj[:half] + f"\n... [{len(obj)} chars truncated] ...\n" + obj[-half:]
        return obj
    return obj


def _strip_verbose_keys(data):
    """Remove keys that are typically verbose and low-value for the LLM.

    Operates on parsed JSON dicts (tool results).
    """
    if not isinstance(data, dict):
        return data

    # Keys that are almost always noise for the model context.
    # NOTE: "hint" and "total_lines" are NOT stripped because they are
    # critical for Hermes read_file/search_files pagination — removing
    # them breaks the LLM's ability to request more content.
    verbose_keys = {
        "truncated", "file_size", "is_binary", "is_image",
        "status", "dedup", "content_returned",  # status metadata, not content
    }

    # Don't strip if the dict is small
    json_len = len(json.dumps(data, ensure_ascii=False))
    if json_len < _COMPRESS_THRESHOLD:
        return data

    stripped = {}
    for k, v in data.items():
        if k in verbose_keys and isinstance(v, (bool, str, int)):
            # Skip metadata booleans and small values
            if isinstance(v, bool):
                continue
            if isinstance(v, str) and len(v) < 50:
                continue
            if isinstance(v, int) and k in ("total_lines", "file_size"):
                continue
        stripped[k] = v

    return stripped if stripped else data


def _compress_tool_result(tool_name, result_str):
    """Compress a tool result string to save tokens.

    For JSON results: truncate long values, strip verbose keys.
    For plain text: truncate with summary.

    Returns the compressed string, or the original if compression
    doesn't help enough.
    """
    if not isinstance(result_str, str) or len(result_str) < _COMPRESS_THRESHOLD:
        return result_str

    original_len = len(result_str)
    compressed = result_str

    # Try JSON parsing and structural compression
    try:
        data = json.loads(result_str)
        if isinstance(data, dict):
            # Strip verbose metadata keys
            data = _strip_verbose_keys(data)
            # Truncate long string values
            data = _truncate_long_values(data)
            compressed = json.dumps(data, ensure_ascii=False)
    except (json.JSONDecodeError, ValueError):
        # Not JSON — apply plain text compression
        # Truncate the middle of very long plain text results
        if len(result_str) > 8000:
            head = result_str[:3000]
            tail = result_str[-2000:]
            lines_total = result_str.count("\n") + 1
            omitted = len(result_str) - len(head) - len(tail)
            compressed = (
                f"{head}\n\n"
                f"... [{lines_total} lines, {omitted} chars omitted] ...\n\n"
                f"{tail}"
            )

    # Only apply if savings are meaningful (>10%)
    if len(compressed) < original_len * 0.9:
        _record_compression(tool_name, original_len, len(compressed))
        return compressed

    return result_str


def register(ctx):
    """Register the Hermes pre-tool and transform hooks."""
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
    """Append RTK savings summary AND compress large non-terminal tool results."""
    try:
        if not isinstance(result, str):
            return result  # don't transform non-string results

        # Step 1: Compress large non-terminal tool results
        if tool_name not in _SKIP_COMPRESS_TOOLS:
            original_len = len(result)
            if original_len >= _COMPRESS_THRESHOLD:
                compressed = _compress_tool_result(tool_name, result)
                if compressed is not None and len(compressed) < original_len:
                    result = compressed

        # Step 2: Append RTK savings summary (from terminal command rewrites)
        saved, count, avg_pct = _read_rtk_savings()

        # Also include compression savings from non-terminal tools
        comp_saved, comp_count, comp_pct = _read_compression_savings()

        # Combine and display
        if saved <= 0 and comp_saved <= 0:
            return None

        parts = []
        if saved > 0 and count > 0:
            parts.append(f"rtk: {_format_tokens(saved)} across {count} cmds (avg {avg_pct}%)")
        if comp_saved > 0 and comp_count > 0:
            parts.append(f"cmp: ~{_format_chars(comp_saved)} across {comp_count} results (avg {comp_pct}%)")

        tag = "\n⟡ " + " | ".join(parts)

        # Try to inject into JSON result
        try:
            data = json.loads(result)
            if isinstance(data, dict) and "output" in data:
                data["output"] = data["output"] + tag
                return json.dumps(data)
        except (json.JSONDecodeError, ValueError):
            pass

        # Fallback: just append to raw string
        return result + tag
    except Exception as e:
        _warn(f"transform_tool_result error: {e}")
        return result  # fail open: return original result, dont drop it


def _warn(message):
    print(f"rtk: hermes plugin warning: {message}", file=sys.stderr)