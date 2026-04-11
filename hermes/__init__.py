"""
RTK Rewrite Plugin for Hermes

Initial Hermes support: transparently rewrites terminal tool commands to RTK equivalents
before execution, achieving 60-90% LLM token savings.

All rewrite logic lives in `rtk rewrite` (src/discover/registry.rs).
This plugin is a thin delegate — to add or change rules, edit the
Rust registry, not this file.
"""

from __future__ import annotations

import logging
import shutil
import subprocess
from typing import Optional

logger = logging.getLogger(__name__)

_rtk_available: Optional[bool] = None
_SUCCESS_REWRITE_EXIT_CODES = {0, 3}


def _check_rtk() -> bool:
    """Check if rtk binary is available in PATH. Result is cached."""
    global _rtk_available
    if _rtk_available is not None:
        return _rtk_available
    _rtk_available = shutil.which("rtk") is not None
    return _rtk_available


def _should_skip(command: str) -> bool:
    """Skip commands that should never be re-rewritten."""
    stripped = command.lstrip()
    return stripped.startswith("rtk ") or "RTK_DISABLED=1" in command


def _try_rewrite(command: str) -> Optional[str]:
    """Delegate to `rtk rewrite` and return the rewritten command, or None."""
    if _should_skip(command):
        return None

    try:
        result = subprocess.run(
            ["rtk", "rewrite", command],
            capture_output=True,
            text=True,
            timeout=2,
        )
        rewritten = result.stdout.strip()
        if result.returncode in _SUCCESS_REWRITE_EXIT_CODES and rewritten and rewritten != command:
            return rewritten
        return None
    except (subprocess.TimeoutExpired, FileNotFoundError, OSError):
        return None



def _pre_tool_call(*, tool_name: str, args: dict, task_id: str, **_kwargs) -> None:
    """pre_tool_call hook: rewrite terminal commands to use RTK.

    Mutates ``args["command"]`` in-place when RTK provides a rewrite.
    The dict is mutable, so changes propagate to the caller without
    needing a return value.
    """
    if tool_name != "terminal":
        return

    command = args.get("command")
    if not isinstance(command, str) or not command.strip():
        return

    rewritten = _try_rewrite(command)
    if rewritten:
        logger.debug("[rtk] %s -> %s", command, rewritten)
        args["command"] = rewritten



def register(ctx) -> None:
    """Entry point called by Hermes plugin system."""
    if not _check_rtk():
        logger.warning("[rtk] rtk binary not found in PATH — plugin disabled")
        return

    ctx.register_hook("pre_tool_call", _pre_tool_call)
    logger.info("[rtk] Hermes plugin registered")
