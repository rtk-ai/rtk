#!/usr/bin/env python3
"""Validate RTK's deterministic agent fixtures and optionally run a typed host command.

The default path is offline and does not start an agent.  ``--live-command`` is
an explicit opt-in subprocess check; its arguments are passed exactly as typed
and the result is reported as host evidence, not as a model benchmark.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path


def validate_manifest(repo: Path) -> dict[str, object]:
    manifest_path = repo / "tests" / "fixtures" / "agent_capabilities.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    cases = manifest.get("cases")
    if manifest.get("schema_version") != 1 or not isinstance(cases, list) or not cases:
        raise ValueError("manifest must have schema_version=1 and a non-empty cases list")

    ids: set[str] = set()
    for case in cases:
        if not isinstance(case, dict):
            raise ValueError("every manifest case must be an object")
        case_id = case.get("id")
        if not isinstance(case_id, str) or not case_id or case_id in ids:
            raise ValueError(f"invalid or duplicate case id: {case_id!r}")
        ids.add(case_id)
        route = case.get("route")
        argv = case.get("argv")
        fixture = case.get("fixture")
        if not isinstance(route, str) or not route:
            raise ValueError(f"{case_id}: route is required")
        if not isinstance(argv, list) or not argv or argv[0] == "rtk":
            raise ValueError(f"{case_id}: argv must be a typed RTK argument vector")
        fixture_path = repo / fixture if isinstance(fixture, str) else Path()
        if isinstance(fixture, str) and not fixture_path.is_file():
            fixture_path = repo / "tests" / "fixtures" / fixture
        if not isinstance(fixture, str) or not fixture_path.is_file():
            raise ValueError(f"{case_id}: fixture file is missing")

    return {"schema_version": 1, "cases": len(cases), "fixture_validation": "passed"}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--live-command",
        nargs=argparse.REMAINDER,
        help="explicitly run a host command after this option; no command is run by default",
    )
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[1]

    try:
        report = validate_manifest(repo)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(json.dumps({"fixture_validation": "failed", "error": str(error)}))
        return 1

    if not args.live_command:
        report["live_verification"] = "unverified"
        report["live_reason"] = "No --live-command was supplied"
    else:
        command = list(args.live_command)
        if command and command[0] == "--":
            command.pop(0)
        if not command:
            print(json.dumps({"fixture_validation": "passed", "live_verification": "invalid"}))
            return 2
        completed = subprocess.run(command, cwd=repo, text=True, capture_output=True, check=False)
        report["live_verification"] = "passed" if completed.returncode == 0 else "failed"
        report["live_command"] = command
        report["live_exit_code"] = completed.returncode
        report["live_stdout_bytes"] = len(completed.stdout.encode("utf-8"))
        report["live_stderr_bytes"] = len(completed.stderr.encode("utf-8"))

    print(json.dumps(report, sort_keys=True))
    return 0 if report["fixture_validation"] == "passed" and report["live_verification"] != "failed" else 1


if __name__ == "__main__":
    sys.exit(main())
