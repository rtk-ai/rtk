"""Emit the curated ruleset as src/core/rules/secrets.toml."""
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

REPO = os.environ.get("RTK_REPO",
                      os.path.abspath(os.path.join(
                          os.path.dirname(__file__), "..", "..")))
OUT = f"{REPO}/src/core/rules/secrets.toml"

HEADER = '''# rtk secret-detection ruleset -- DO NOT EDIT BY HAND.
#
# Regenerate with:  python3 scripts/secret-rules/build.py
#
# Provenance
# ----------
# Patterns derive from gitleaks (https://github.com/gitleaks/gitleaks), MIT.
# See NOTICE for the licence text. rtk is Apache-2.0; MIT is compatible.
#
# What was changed and why
# ------------------------
# gitleaks scans *repositories*: a high-entropy blob sitting in a file near a
# vendor keyword is suspicious. rtk scans *command output*, where the same blob
# is a commit SHA or a lockfile checksum. Redacting one of those silently
# corrupts what the agent reasons about -- a worse outcome than the leak,
# because it is invisible. So this set is curated, not imported wholesale:
#
#   * 222 upstream rules -> {n} kept. Rules whose only keyword is the vendor's
#     own name ("twitter", "airtable") are dropped -- not because they cannot
#     fire on output (they can: HEROKU_API_KEY=... appears in `rtk env`) but
#     because 70 per-vendor rules are the wrong shape. A KEY=VALUE line is
#     identified by its key name, so ~40 generic key-name rules cover every
#     vendor at a fraction of the anchor cost. See src/core/rules/README.md.
#   * Anchors must be >=3 chars, or 2 chars including punctuation. Short
#     anchors defeat the aho-corasick prefilter that keeps rtk under its 10ms
#     budget.
#   * Two patterns are narrowed vs upstream; each says why inline.
#
# Every rule carries its own `positives` and `negatives`, so the ruleset is
# self-testing (see tests/secret_rules.rs). A rule with no positive does not
# ship.
#
# Fields
# ------
#   id           upstream rule id, kept for traceability
#   class        A = self-identifying prefix, B = structural shape
#   label        appears in the placeholder: [REDACTED:<label>:<hash>]
#   anchors      literals for the aho-corasick prefilter (lowercased)
#   pattern      RE2 syntax; lookaround/backref free, so the regex crate
#                accepts it unchanged
#   entropy_min  Shannon floor for the captured group, when upstream sets one
#   allowlist_*  upstream FP suppressors: known-fake values that must not be
#                treated as secrets (_secret tests the capture, _match the
#                whole match)
#   positives_hex  must match, hex-encoded (see below)
#   negatives_hex  must not match, hex-encoded
#
# Fixtures are hex-encoded, with a truncated plaintext preview above each.
# They are synthetic, but shaped exactly like real credentials -- so GitHub's
# push protection reads them as live secrets and blocks the push, and every
# fork would raise a secret-scanning alert. Encoding keeps the repo clean
# without weakening the fixture. tests/secret_rules.rs decodes them.

version = {v}
'''


def lit(s: str) -> str:
    """TOML multi-line literal -- verbatim, no escape soup in the regexes."""
    assert "'''" not in s, s
    return f"'''{s}'''"


def hexed(s: str) -> str:
    """Hex-encode a fixture.

    These are synthetic, but they are shaped exactly like real credentials --
    which is the point -- so GitHub's push protection classifies them as live
    secrets and refuses the push. Committing them would also fire a
    secret-scanning alert in every fork of the repo.

    Upstream avoids this by generating its samples at run time and committing
    almost none; we keep committed fixtures for reviewability and encode them
    instead. Hex rather than base64: some scanners decode base64, none decode
    hex, and the cost is only file size.
    """
    return s.encode("utf-8").hex()


def preview(s: str) -> str:
    """A short plaintext hint so the file stays reviewable.

    Capped well below the length any rule needs to match, so the preview
    itself can never be mistaken for a credential.
    """
    head = "".join(c if " " <= c < "\x7f" else "." for c in s[:12])
    return f"{head}... ({len(s)} chars)"


def basic(s: str) -> str:
    """TOML basic string. Control characters must be escaped, not emitted raw --
    generated samples can contain \\x0b / \\x0c from boundary classes."""
    out = []
    for ch in s:
        if ch == '\\':
            out.append('\\\\')
        elif ch == '"':
            out.append('\\"')
        elif ch == '\n':
            out.append('\\n')
        elif ch == '\r':
            out.append('\\r')
        elif ch == '\t':
            out.append('\\t')
        elif ord(ch) < 0x20 or ord(ch) == 0x7F:
            out.append(f'\\u{ord(ch):04X}')
        else:
            out.append(ch)
    return '"' + ''.join(out) + '"'


def main():
    rules = json.load(open('curated.json'))
    rules.sort(key=lambda r: (r['category'], r['id']))

    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    parts = [HEADER.format(n=len(rules), v=1)]
    cat = None
    for r in rules:
        if r['category'] != cat:
            cat = r['category']
            parts.append(f"\n# {'=' * 70}\n# {cat}\n# {'=' * 70}")
        parts.append("\n[[rule]]")
        parts.append(f"id      = {basic(r['id'])}")
        parts.append(f"class   = {basic(r['class'])}")
        parts.append(f"label   = {basic(r['label'])}")
        parts.append("anchors = [" +
                     ", ".join(basic(a) for a in r['anchors']) + "]")
        parts.append(f"pattern = {lit(r['pattern'])}")
        if r.get('entropy_min'):
            parts.append(f"entropy_min = {float(r['entropy_min'])}")
        if r.get('secret_group') is not None:
            parts.append(f"secret_group = {int(r['secret_group'])}")
        for field in ('allowlist_secret', 'allowlist_match'):
            vals = r.get(field) or []
            if vals:
                parts.append(f"{field} = [")
                for v in vals:
                    parts.append(f"  {lit(v)},")
                parts.append("]")
        if r.get('narrowed'):
            parts.append("narrowed = true  # see PATTERN_OVERRIDES in build.py")
        parts.append("positives_hex = [")
        for p in r['positives']:
            parts.append(f"  # {preview(p)}")
            parts.append(f"  {basic(hexed(p))},")
        parts.append("]")
        if r['negatives']:
            parts.append("negatives_hex = [")
            for n_ in r['negatives']:
                parts.append(f"  # {preview(n_)}")
                parts.append(f"  {basic(hexed(n_))},")
            parts.append("]")
        else:
            parts.append("negatives_hex = []")

    open(OUT, 'w').write("\n".join(parts) + "\n")
    size = os.path.getsize(OUT)
    print(f"wrote {OUT}  ({size:,} bytes, {len(rules)} rules)")

    # round-trip check
    import tomllib
    d = tomllib.load(open(OUT, 'rb'))
    assert len(d['rule']) == len(rules), "round-trip lost rules"
    print(f"round-trip OK: {len(d['rule'])} rules parse back")


if __name__ == '__main__':
    sys.exit(main())
