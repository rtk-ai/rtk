#!/usr/bin/env python3
"""Regenerate src/core/rules/secrets.toml from the gitleaks corpus.

Pipeline:
  1. fetch    gitleaks master (config/gitleaks.toml + Go rule sources)
  2. extract  per-rule true/false-positive fixtures from the Go sources
  3. build    apply rtk's inclusion criteria, synthesise missing positives
  4. corpus   rebuild the negative corpus from this repo's own output
  5. emit     write secrets.toml
  6. validate recall / self-FP / precision gates

Usage:
    python3 scripts/secret-rules/build.py [--skip-fetch] [--skip-corpus]

Upstream is pinned to a commit SHA (see GITLEAKS_REF) and the config file is
checksummed, so a regeneration is reproducible rather than "whatever master
said today".

Requires python3 >= 3.11 (tomllib) and network access for step 1.
The authoritative gate is `cargo test --test secret_rules`; this script is the
fast iteration loop.
"""
import argparse
import hashlib
import os
import shutil
import subprocess
import sys
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
WORK = os.path.join(HERE, ".work")

# Pinned upstream: a commit SHA, not a branch.
#
# `master` made regeneration non-deterministic -- two runs a month apart
# produce different rulesets from the same command, and the NOTICE attribution
# points at whatever the branch happened to be that day. A SHA is immutable, so
# `secrets.toml` is reproducible from this file alone.
#
# Bumping it is a deliberate act: change GITLEAKS_REF, run with --update-pin to
# print the new digest, paste it below, then re-run the gates. Record the
# version in NOTICE too.
GITLEAKS_REF = "83d9cd684c87d95d656c1458ef04895a7f1cbd8e"  # tag v8.30.1
GITLEAKS_VERSION = "v8.30.1"
# sha256 of config/gitleaks.toml at GITLEAKS_REF. Content at a SHA cannot
# change, so a mismatch means the download was tampered with or truncated.
GITLEAKS_TOML_SHA256 = \
    "e163e53b9e7e8a8511e77271e2b323ed057759542a6d988258afe3a1fa329caf"

GITLEAKS_TOML = ("https://raw.githubusercontent.com/gitleaks/gitleaks/"
                 f"{GITLEAKS_REF}/config/gitleaks.toml")
GITLEAKS_TAR = ("https://github.com/gitleaks/gitleaks/archive/"
                f"{GITLEAKS_REF}.tar.gz")

# The archive unpacks to gitleaks-<ref>; normalise it so the downstream scripts
# do not have to know which ref was pinned.
SRC_DIR = "gitleaks-src"


def run(script, *args):
    print(f"\n--- {script} ---", flush=True)
    r = subprocess.run([sys.executable, os.path.join(HERE, script), *args],
                       cwd=WORK, env={**os.environ, "RTK_REPO": REPO})
    if r.returncode:
        sys.exit(f"{script} failed with {r.returncode}")


def fetch(update_pin=False):
    os.makedirs(WORK, exist_ok=True)

    cfg = os.path.join(WORK, "gitleaks.toml")
    print(f"fetching {GITLEAKS_TOML}", flush=True)
    urllib.request.urlretrieve(GITLEAKS_TOML, cfg)

    digest = hashlib.sha256(open(cfg, "rb").read()).hexdigest()
    if update_pin:
        print(f"\n  GITLEAKS_TOML_SHA256 = \\\n      \"{digest}\"\n"
              f"  paste that into build.py, then re-run without --update-pin")
    elif digest != GITLEAKS_TOML_SHA256:
        sys.exit(f"gitleaks.toml digest mismatch at {GITLEAKS_REF}\n"
                 f"  expected {GITLEAKS_TOML_SHA256}\n"
                 f"  got      {digest}\n"
                 f"Content at a commit SHA is immutable, so this is a bad "
                 f"download or a tampered one -- not an upstream change. "
                 f"Re-run; if it persists, investigate before regenerating.")

    tar = os.path.join(WORK, "gl.tar.gz")
    print(f"fetching {GITLEAKS_TAR}", flush=True)
    urllib.request.urlretrieve(GITLEAKS_TAR, tar)
    # Not checksummed: GitHub's generated archives are not byte-stable over
    # time. The SHA in the URL is what guarantees the contents.
    subprocess.run(["tar", "xzf", tar], cwd=WORK, check=True)

    unpacked = os.path.join(WORK, f"gitleaks-{GITLEAKS_REF}")
    if not os.path.isdir(unpacked):
        sys.exit(f"archive did not unpack to {unpacked}")
    dest = os.path.join(WORK, SRC_DIR)
    shutil.rmtree(dest, ignore_errors=True)
    os.rename(unpacked, dest)

    import json
    import tomllib
    d = tomllib.load(open(cfg, "rb"))
    json.dump(d["rules"], open(os.path.join(WORK, "gitleaks_rules.json"), "w"))
    print(f"  gitleaks {GITLEAKS_VERSION} ({GITLEAKS_REF[:12]}) -- "
          f"{len(d['rules'])} upstream rules")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--skip-fetch", action="store_true")
    ap.add_argument("--skip-corpus", action="store_true")
    ap.add_argument("--update-pin", action="store_true",
                    help="print the digest for a newly bumped GITLEAKS_REF "
                         "instead of verifying against the recorded one")
    a = ap.parse_args()

    os.makedirs(WORK, exist_ok=True)
    if not a.skip_fetch:
        fetch(update_pin=a.update_pin)
        if a.update_pin:
            return 0
    run("extract_tps.py")
    run("build_rules.py")
    if not a.skip_corpus:
        run("build_corpus.py")
    run("emit_toml.py")
    run("validate.py")
    print("\nOK -- now run: cargo test --test secret_rules")


if __name__ == "__main__":
    sys.exit(main())
