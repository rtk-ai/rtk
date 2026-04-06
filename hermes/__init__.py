"""
RTK Token Optimizer plugin for Hermes.

Intercepts terminal tool calls and rewrites commands via `rtk rewrite`
to reduce LLM token consumption on common dev commands.

This is a Hermes-specific adaptation of RTK's upstream OpenClaw/Claude Code
integrations. Rewrite logic remains centralized in the RTK binary.
"""

from __future__ import annotations

import logging
import os
import re
import shutil
import subprocess
from collections import OrderedDict
from dataclasses import dataclass
from typing import Literal

logger = logging.getLogger(__name__)

_MIN_RTK_VERSION = (0, 23, 0)
_COMPOUND_COMMAND_PATTERN = re.compile(r"\|\||&&|\||;|<<")


@dataclass(frozen=True)
class _Config:
    enabled: bool = True
    verbose: bool = False
    timeout_ms: int = 2000
    skip_background: bool = True
    skip_pty: bool = True
    cache_size: int = 256


@dataclass(frozen=True)
class _RewriteResult:
    action: Literal["rewrite", "pass", "ask", "deny"]
    command: str | None = None
    reason: str = ""


_rtk_checked = False
_rtk_available = False
_rtk_version: tuple[int, int, int] | None = None
_config: _Config | None = None
_cache: OrderedDict[str, _RewriteResult] = OrderedDict()


def _env_bool(name: str, default: bool) -> bool:
    value = os.getenv(name)
    if value is None:
        return default
    return value.strip().lower() in {"1", "true", "yes", "on"}


def _env_int(name: str, default: int, minimum: int) -> int:
    value = os.getenv(name)
    if value is None:
        return default
    try:
        parsed = int(value)
    except ValueError:
        logger.warning("[rtk] invalid integer for %s=%r; using %s", name, value, default)
        return default
    return max(parsed, minimum)


def _load_config() -> _Config:
    global _config
    if _config is None:
        _config = _Config(
            enabled=_env_bool("HERMES_RTK_ENABLED", True),
            verbose=_env_bool("HERMES_RTK_VERBOSE", False),
            timeout_ms=_env_int("HERMES_RTK_TIMEOUT_MS", 2000, 100),
            skip_background=_env_bool("HERMES_RTK_SKIP_BACKGROUND", True),
            skip_pty=_env_bool("HERMES_RTK_SKIP_PTY", True),
            cache_size=_env_int("HERMES_RTK_CACHE_SIZE", 256, 0),
        )
    return _config


def _parse_version(text: str) -> tuple[int, int, int] | None:
    match = re.search(r"(\d+)\.(\d+)\.(\d+)", text)
    if not match:
        return None
    return tuple(int(part) for part in match.groups())


def _version_str(version: tuple[int, int, int] | None) -> str:
    if version is None:
        return "unknown"
    return ".".join(str(part) for part in version)


def _check_rtk() -> bool:
    global _rtk_checked, _rtk_available, _rtk_version
    if _rtk_checked:
        return _rtk_available

    _rtk_checked = True
    rtk_path = shutil.which("rtk")
    if not rtk_path:
        logger.warning("[rtk] rtk binary not found in PATH — plugin disabled")
        return False

    try:
        result = subprocess.run(
            ["rtk", "--version"],
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )
    except Exception as exc:
        logger.warning("[rtk] unable to check rtk version (%s) — plugin disabled", exc)
        return False

    version_text = (result.stdout or result.stderr or "").strip()
    _rtk_version = _parse_version(version_text)
    if _rtk_version is None:
        logger.warning("[rtk] could not parse rtk version from %r — plugin disabled", version_text)
        return False

    if _rtk_version < _MIN_RTK_VERSION:
        logger.warning(
            "[rtk] rtk %s is too old; need >= %s — plugin disabled",
            _version_str(_rtk_version),
            _version_str(_MIN_RTK_VERSION),
        )
        return False

    _rtk_available = True
    return True


def _should_skip(command: str, args: dict, config: _Config) -> str | None:
    stripped = command.strip()
    if not stripped:
        return "empty command"
    if stripped.startswith("rtk ") or stripped == "rtk":
        return "already using rtk"
    if _COMPOUND_COMMAND_PATTERN.search(command):
        return "compound shell command"
    if config.skip_background and bool(args.get("background")):
        return "background terminal call"
    if config.skip_pty and bool(args.get("pty")):
        return "pty terminal call"
    return None


def _cache_get(command: str) -> _RewriteResult | None:
    cached = _cache.get(command)
    if cached is None:
        return None
    _cache.move_to_end(command)
    return cached


def _cache_put(command: str, result: _RewriteResult, config: _Config) -> None:
    if config.cache_size <= 0:
        return
    _cache[command] = result
    _cache.move_to_end(command)
    while len(_cache) > config.cache_size:
        _cache.popitem(last=False)


def _try_rewrite(command: str, config: _Config) -> _RewriteResult:
    cached = _cache_get(command)
    if cached is not None:
        return cached

    try:
        result = subprocess.run(
            ["rtk", "rewrite", command],
            capture_output=True,
            text=True,
            timeout=max(config.timeout_ms / 1000.0, 0.1),
            check=False,
        )
    except subprocess.TimeoutExpired:
        rewrite_result = _RewriteResult(action="pass", reason="timeout")
        _cache_put(command, rewrite_result, config)
        return rewrite_result
    except Exception as exc:
        rewrite_result = _RewriteResult(action="pass", reason=f"error:{exc}")
        _cache_put(command, rewrite_result, config)
        return rewrite_result

    rewritten = (result.stdout or "").strip()
    if result.returncode == 0:
        if rewritten and rewritten != command:
            rewrite_result = _RewriteResult(action="rewrite", command=rewritten, reason="rewritten")
        else:
            rewrite_result = _RewriteResult(action="pass", reason="identical output")
    elif result.returncode == 1:
        rewrite_result = _RewriteResult(action="pass", reason="no RTK equivalent")
    elif result.returncode == 2:
        rewrite_result = _RewriteResult(action="deny", reason="rtk deny rule")
    elif result.returncode == 3:
        rewrite_result = _RewriteResult(action="ask", command=rewritten or None, reason="rtk ask rule")
    else:
        rewrite_result = _RewriteResult(action="pass", reason=f"unexpected exit {result.returncode}")

    _cache_put(command, rewrite_result, config)
    return rewrite_result


def _on_pre_tool_call(tool_name: str, args: dict, task_id: str, **kwargs) -> None:
    del task_id, kwargs

    if tool_name != "terminal":
        return

    config = _load_config()
    if not config.enabled:
        return
    if not _check_rtk():
        return

    command = args.get("command", "")
    if not isinstance(command, str):
        return

    skip_reason = _should_skip(command, args, config)
    if skip_reason:
        if config.verbose:
            logger.info("[rtk] skipped rewrite for %r (%s)", command, skip_reason)
        return

    rewrite = _try_rewrite(command, config)

    if rewrite.action == "rewrite" and rewrite.command:
        args["command"] = rewrite.command
        if config.verbose:
            logger.info("[rtk] %s -> %s", command, rewrite.command)
        return

    if rewrite.action == "ask":
        logger.info(
            "[rtk] ask-rule matched for %r; leaving command unchanged because Hermes pre_tool_call cannot trigger an interactive confirmation prompt",
            command,
        )
        return

    if config.verbose:
        logger.info("[rtk] no rewrite for %r (%s)", command, rewrite.reason)


def register(ctx) -> None:
    config = _load_config()
    if not config.enabled:
        logger.info("[rtk] plugin disabled via HERMES_RTK_ENABLED")
        return

    if not _check_rtk():
        logger.warning("[rtk] install/upgrade RTK from https://github.com/rtk-ai/rtk")
        return

    ctx.register_hook("pre_tool_call", _on_pre_tool_call)
    logger.info(
        "[rtk] RTK token optimizer plugin v1.1 loaded (terminal hook registered, rtk=%s, timeout=%sms)",
        _version_str(_rtk_version),
        config.timeout_ms,
    )
