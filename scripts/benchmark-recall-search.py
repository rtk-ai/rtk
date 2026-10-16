#!/usr/bin/env python3
"""Compare many-file search recovery costs on isolated, reproducible data.

Example: python3 scripts/benchmark-recall-search.py /tmp/rtk-before target/release/rtk
SQLite raw minus disabled approximates hashing/storage overhead; SQLite gzip minus
raw isolates the incremental compression cost (including its smaller writes).
No network, third-party Python packages, or changes to the user's RTK stores.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import statistics
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binaries", nargs="+", type=Path)
    parser.add_argument("--files", type=int, default=100)
    parser.add_argument("--lines", type=int, default=80)
    parser.add_argument("--runs", type=int, default=20)
    parser.add_argument("--warmup", type=int, default=3)
    parser.add_argument("--max", type=int, help="RTK display cap (default: 25 * files)")
    args = parser.parse_args()
    if min(args.files, args.lines, args.runs) < 1 or args.warmup < 0:
        parser.error("files, lines and runs must be positive; warmup must be nonnegative")
    max_results = args.max if args.max is not None else args.files * 25
    modes = [("disabled", False), ("tee", False), ("sqlite", False), ("sqlite", True)]
    results = {}
    with tempfile.TemporaryDirectory(prefix="rtk-recall-bench-") as temp:
        root = Path(temp)
        corpus = root / "corpus"
        corpus.mkdir()
        for i in range(args.files):
            (corpus / f"file_{i:04}.txt").write_text("".join(
                f"needle file={i:04} line={j:04} deterministic search benchmark payload\n"
                for j in range(args.lines)
            ))
        cases = []
        for binary in args.binaries:
            binary = binary.resolve(strict=True)
            for mode, compression in modes:
                key = f"{binary}:{mode}" + ("-gzip" if compression else "-raw")
                home = root / f"case-{len(cases)}"
                config = home / ".config" / "rtk"
                config.mkdir(parents=True)
                (config / "config.toml").write_text(
                    f'[retriever]\nmode = "{mode}"\ncompression = {str(compression).lower()}\n'
                    '[hooks]\nsuppress_hook_warning = true\n[telemetry]\nenabled = false\n'
                )
                env = {k: v for k, v in os.environ.items() if not k.startswith("RTK_")}
                env.update(HOME=str(home), XDG_CONFIG_HOME=str(home / ".config"),
                           XDG_DATA_HOME=str(home / ".local/share"), RTK_TELEMETRY_DISABLED="1",
                           RTK_RECALL_DB=str(home / "recall.db"), RTK_TEE_DIR=str(home / "tee"),
                           LC_ALL="C", NO_COLOR="1")
                command = [str(binary), "grep", "--max", str(max_results),
                           "-rn", "needle", "corpus"]
                cases.append((key, command, env))
                results[key] = {"milliseconds": []}
        rng = random.Random(3946)
        for iteration in range(args.warmup + args.runs):
            rng.shuffle(cases)
            for key, command, env in cases:
                start = time.perf_counter_ns()
                run = subprocess.run(command, cwd=root, env=env, stdin=subprocess.DEVNULL,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True)
                elapsed = (time.perf_counter_ns() - start) / 1_000_000
                if iteration >= args.warmup:
                    results[key]["milliseconds"].append(elapsed)
                results[key]["output_sha256"] = hashlib.sha256(run.stdout).hexdigest()
                results[key]["output_bytes"] = len(run.stdout)
                results[key]["recall_hints"] = run.stdout.count(b"rtk recall ")
        for result in results.values():
            times = result["milliseconds"]
            result.update(mean_ms=statistics.mean(times), median_ms=statistics.median(times),
                          stdev_ms=statistics.stdev(times) if len(times) > 1 else 0)
        print(json.dumps({"files": args.files, "lines_per_file": args.lines,
                          "max_results": max_results,
                          "runs": args.runs, "warmup": args.warmup,
                          "results": results}, indent=2))


if __name__ == "__main__":
    main()
