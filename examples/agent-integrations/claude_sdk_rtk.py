"""Small SDK-worker adapter sketch; no network or API calls are made here."""

from __future__ import annotations

import os
import subprocess
from dataclasses import dataclass


@dataclass(frozen=True)
class RtkResult:
    stdout: str
    stderr: str
    exit_code: int


def run_rtk(args: list[str], *, cwd: str, max_tokens: int = 2048) -> RtkResult:
    """Run a typed RTK route in the worker environment with a local budget."""
    environment = os.environ.copy()
    environment["RTK_MAX_OUTPUT_TOKENS"] = str(max_tokens)
    completed = subprocess.run(
        ["rtk", *args],
        cwd=cwd,
        env=environment,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        check=False,
        shell=False,
    )
    return RtkResult(completed.stdout, completed.stderr, completed.returncode)


# SDK tool callback sketch:
# result = run_rtk(["git", "status"], cwd=os.getcwd())
# return {"content": [{"type": "text", "text": result.stdout}],
#         "is_error": result.exit_code != 0}
