#!/usr/bin/env python3
"""Measure paired model-visible output without contacting an agent service.

The script intentionally reports byte_estimate unless an optional tokenizer is
requested. It measures producer/model-input/recovery/context fields separately;
it does not claim that a byte estimate is a billing-token count.
"""

from __future__ import annotations

import argparse
import json
import shlex
import subprocess
from pathlib import Path
from typing import Any, Callable


def run_command(command: str | None) -> tuple[bytes, int]:
    if not command:
        return b"", 0
    completed = subprocess.run(
        shlex.split(command, posix=False),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    return completed.stdout, completed.returncode


def counter_for(name: str) -> tuple[str, Callable[[bytes], int]]:
    if name == "byte_estimate":
        return name, lambda value: (len(value) + 3) // 4
    if name.startswith("tiktoken:"):
        encoding_name = name.split(":", 1)[1]
        try:
            import tiktoken  # type: ignore
        except ImportError as error:
            raise SystemExit(
                "tiktoken counter requested but the optional tiktoken package is unavailable"
            ) from error
        encoding = tiktoken.get_encoding(encoding_name)
        return name, lambda value: len(encoding.encode(value.decode("utf-8", "replace")))
    raise SystemExit(f"unsupported counter: {name}")


def measurement(
    *,
    label: str,
    raw: bytes,
    baseline: bytes,
    candidate: bytes,
    recovery: bytes,
    hook_context: bytes,
    tool_schema_context: bytes,
    baseline_exit: int,
    candidate_exit: int,
    counter_name: str,
    counter: Callable[[bytes], int],
) -> dict[str, Any]:
    return {
        "label": label,
        "counter": counter_name,
        "raw_producer": {
            "bytes": len(raw),
            "tokens": counter(raw),
        },
        "baseline_model_input": {
            "bytes": len(baseline),
            "tokens": counter(baseline),
        },
        "candidate_model_input": {
            "bytes": len(candidate),
            "tokens": counter(candidate),
        },
        "recovery_input": {
            "bytes": len(recovery),
            "tokens": counter(recovery),
        },
        "hook_context": {
            "bytes": len(hook_context),
            "tokens": counter(hook_context),
        },
        "tool_schema_context": {
            "bytes": len(tool_schema_context),
            "tokens": counter(tool_schema_context),
        },
        "baseline_exit": baseline_exit,
        "candidate_exit": candidate_exit,
        "candidate_reduction_pct": (
            (len(baseline) - len(candidate)) / len(baseline) * 100
            if baseline
            else 0.0
        ),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--label", default="paired-output")
    parser.add_argument("--raw-file", type=Path)
    parser.add_argument("--baseline-file", type=Path)
    parser.add_argument("--candidate-file", type=Path)
    parser.add_argument("--recovery-file", type=Path)
    parser.add_argument("--hook-context-file", type=Path)
    parser.add_argument("--tool-schema-file", type=Path)
    parser.add_argument("--baseline-command")
    parser.add_argument("--candidate-command")
    parser.add_argument("--counter", default="byte_estimate")
    args = parser.parse_args()

    name, counter = counter_for(args.counter)

    def read(path: Path | None) -> bytes:
        return path.read_bytes() if path else b""

    baseline_command_output, baseline_exit = run_command(args.baseline_command)
    candidate_command_output, candidate_exit = run_command(args.candidate_command)
    baseline = baseline_command_output or read(args.baseline_file)
    candidate = candidate_command_output or read(args.candidate_file)
    raw = read(args.raw_file) or baseline
    result = measurement(
        label=args.label,
        raw=raw,
        baseline=baseline,
        candidate=candidate,
        recovery=read(args.recovery_file),
        hook_context=read(args.hook_context_file),
        tool_schema_context=read(args.tool_schema_file),
        baseline_exit=baseline_exit,
        candidate_exit=candidate_exit,
        counter_name=name,
        counter=counter,
    )
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
