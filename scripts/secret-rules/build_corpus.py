"""Build the shared negative corpus: real developer-tool output that must
never be redacted.

gitleaks' own false-positives answer "don't alert the developer". rtk needs a
different question answered: "don't corrupt the agent's input". A redactor that
eats a git SHA or a Cargo.lock checksum silently poisons everything the model
reasons about downstream -- worse than the leak it prevents, because it's
invisible. So the corpus is drawn from output rtk actually wraps.
"""
import json
import re
import subprocess
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

# Pathspecs excluded from every git-derived corpus section: the ruleset and
# its fixtures are full of deliberate secrets by construction.
OWN_PATHS = (
    "':(exclude)src/core/rules' "
    "':(exclude)tests/fixtures/secrets' "
    "':(exclude)scripts/secret-rules'"
)

REPO = os.environ.get("RTK_REPO",
                      os.path.abspath(os.path.join(
                          os.path.dirname(__file__), "..", "..")))


def sh(cmd, cwd=REPO):
    """Capture a command's stdout, failing loudly if it produces none.

    Swallowing the error here would be worse than useless: a git command that
    fails returns an empty corpus section, the precision gate then has nothing
    to run against, and the whole thing reports PASS. A silently empty corpus
    looks exactly like a clean one.
    """
    r = subprocess.run(cmd, shell=True, cwd=cwd, capture_output=True,
                       text=True, timeout=60)
    if r.returncode != 0 or not r.stdout.strip():
        sys.exit(f"corpus section command produced nothing:\n  {cmd}\n"
                 f"  exit={r.returncode}  stderr={r.stderr.strip()[:400]}")
    return r.stdout


def main():
    corpus = {}

    # 1. Cargo.lock -- 64-char hex checksums, the classic entropy trap.
    corpus['cargo_lock'] = open(f"{REPO}/Cargo.lock").read()[:60000]

    # 2. git SHAs, in both raw and log form.
    corpus['git_sha_list'] = sh("git log --format=%H -n 400")
    corpus['git_log'] = sh(f"git log -n 60 --stat -- . {OWN_PATHS}")

    # 3. A real diff -- long base64-ish blobs, +/- noise.
    #
    # Our own paths are excluded. Once the ruleset is committed, `git log -p`
    # replays the positive fixtures back into the corpus and every rule
    # "fires" on its own test data -- a self-poisoning negative corpus that
    # fails the precision gate for the wrong reason.
    corpus['git_diff'] = sh(f"git log -p -n 8 -- . {OWN_PATHS}")[:60000]

    # 4. Container digests: `sha256:` is one char from openshift's `sha256~`.
    corpus['digests'] = "\n".join(
        f"ghcr.io/acme/svc@sha256:{'0123456789abcdef' * 4} {i}"
        for i in range(40))

    # 5. npm integrity hashes (base64 SHA-512) and UUIDs.
    corpus['npm_integrity'] = "\n".join(
        f'"integrity": "sha512-{"AbCdEf0123456789+/" * 5}=="' for _ in range(40))
    corpus['uuids'] = "\n".join(
        f"550e8400-e29b-41d4-a716-{i:012d}" for i in range(200))

    # 6. Base64 payload (an inlined image is the usual shape).
    corpus['base64_blob'] = "data:image/png;base64," + ("iVBORw0KGgoAAAANSUhEUg"
                                                        "AAAAEAAAABCAYAAAAfFcSJ") * 60

    # 7. rtk's own source + docs: prose, code, and lots of example strings.
    corpus['rtk_readme'] = open(f"{REPO}/README.md").read()
    corpus['rtk_source'] = sh("cat src/core/stream.rs src/core/runner.rs "
                              "src/cmds/system/read.rs src/cmds/system/env_cmd.rs")

    # 8. Ordinary English prose -- guards against short/dictionary anchors.
    corpus['prose'] = (
        "They said the key money survey conveys a journey through the valley. "
        "Every attorney and monkey obeyed the honey survey key. "
    ) * 120

    json.dump(corpus, open('corpus.json', 'w'))
    total = sum(len(v) for v in corpus.values())
    print(f"corpus sections: {len(corpus)}")
    for k, v in corpus.items():
        print(f"  {k:<16} {len(v):>8,} bytes  {len(v.splitlines()):>6} lines")
    print(f"  {'TOTAL':<16} {total:>8,} bytes")


if __name__ == '__main__':
    sys.exit(main())
