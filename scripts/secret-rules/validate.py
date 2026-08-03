"""Validate the curated ruleset.

Four gates:
  1. RECALL   - every positive fixture must match its own rule.
  2. SELF-FP  - every imported negative must NOT match its own rule.
  3. PRECISION- no rule may fire anywhere in the shared negative corpus of real
                developer-tool output. This is the hard gate: recall is
                negotiable, precision is not.
  4. ALLOWLIST- every suppressor must compile. One that does not fails open.

Also reports anchor selectivity, the proxy for the aho-corasick prefilter cost.

This is the fast loop, not the authority: it runs patterns through a Go->Python
shim, so a pattern can pass here and be rejected by the regex crate rtk ships.
`cargo test --test secret_rules` is what decides.
"""
import json
import math
import re
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from build_rules import pyre


def shannon(s):
    if not s:
        return 0.0
    return -sum((c := s.count(ch) / len(s)) and c * math.log2(c) for ch in set(s))


def secret_of(r, m):
    """The substring a redactor would replace, for one match.

    `secret_group` when the rule sets one -- it is 0 for the two rules whose
    group 1 is an internal fragment -- else group 1, else the whole match.
    Defaulting to group 1 unconditionally made this loop disagree with the
    authoritative gate on exactly those two rules.
    """
    idx = r.get('secret_group')
    if idx is None:
        idx = 1 if m.lastindex else 0
    try:
        grp = m.group(idx)
    except IndexError:
        grp = m.group(0)
    return m.group(0) if grp is None else grp


def firing(r, rx, text):
    """Every match in `text` that would actually be redacted.

    Mirrors `Compiled::firing` in tests/secret_rules.rs: a match counts only
    once it clears the rule's entropy floor and survives its allowlists. Every
    gate below uses this, so "matches the pattern" and "would be redacted"
    cannot drift apart -- a negative may legitimately match the pattern and be
    suppressed by an allowlist, which a bare `rx.search` reads as a failure.
    """
    out = []
    ent_min = r.get('entropy_min')
    for m in rx.finditer(text):
        grp = secret_of(r, m)
        if ent_min is not None and shannon(grp) < float(ent_min):
            continue
        if any(a.search(grp) for a in r['_allow_secret']):
            continue
        if any(a.search(m.group(0)) for a in r['_allow_match']):
            continue
        out.append(grp)
    return out


def main():
    rules = json.load(open('curated.json'))

    # Fixtures are hex in the emitted TOML; curated.json still holds plaintext.
    # Decode defensively so this works against either shape.
    def fixtures(r, key):
        if key in r:
            return r[key]
        return [bytes.fromhex(h).decode('utf-8') for h in r.get(key + '_hex', [])]

    for r in rules:
        r['positives'] = fixtures(r, 'positives')
        r['negatives'] = fixtures(r, 'negatives')
    corpus = json.load(open('corpus.json'))

    fail_recall, fail_selffp, fail_precision = [], [], []
    fail_compile = []
    compiled = []

    for r in rules:
        rx = pyre(r['pattern'])
        # Allowlists are regexes too, and one that fails to compile fails
        # *open* -- the suppressor silently stops suppressing. Report it here
        # rather than dropping it; `every_allowlist_compiles_with_the_regex_
        # crate` is the authoritative version of this check.
        for field in ('allowlist_secret', 'allowlist_match'):
            key = '_allow_' + field.split('_', 1)[1]
            out = []
            for p in r.get(field, []):
                try:
                    out.append(pyre(p))
                except re.error as e:
                    fail_compile.append((r['id'], field, p[:70], str(e)))
            r[key] = out
        compiled.append((r, rx))
        for p in r['positives']:
            if not firing(r, rx, p):
                fail_recall.append((r['id'], p[:60]))
        for n in r['negatives']:
            if firing(r, rx, n):
                fail_selffp.append((r['id'], n[:60]))

    # ---- precision gate over real output -------------------------------
    for section, text in corpus.items():
        for r, rx in compiled:
            hits = firing(r, rx, text)
            if hits:
                fail_precision.append((r['id'], section, hits[0][:70]))

    # ---- anchor selectivity -------------------------------------------
    all_text = "\n".join(corpus.values()).lower()
    anchor_hits = {}
    for r in rules:
        hits = sum(all_text.count(a) for a in r['anchors'])
        if hits:
            anchor_hits[r['id']] = (hits, r['anchors'])

    n = len(rules)
    print(f"rules: {n}   corpus: {len(all_text):,} bytes\n")
    print(f"1. RECALL     : {n - len({x[0] for x in fail_recall})}/{n} rules ok"
          f"   ({len(fail_recall)} bad fixtures)")
    for x in fail_recall[:10]:
        print("     MISS", x)
    print(f"2. SELF-FP    : {len(fail_selffp)} imported negatives wrongly match")
    for x in fail_selffp[:10]:
        print("     HIT ", x)
    print(f"3. PRECISION  : {len(fail_precision)} rule/section hits on real output")
    for x in fail_precision[:20]:
        print(f"     FIRE  {x[0]:<34} in {x[1]:<14} -> {x[2]!r}")
    print(f"4. ALLOWLISTS : {len(fail_compile)} uncompilable (these fail OPEN)")
    for rid, field, pat, err in fail_compile[:10]:
        print(f"     BAD   {rid:<34} [{field}] {pat!r}\n           {err}")

    print(f"\nanchor prefilter: {len(anchor_hits)}/{n} rules have an anchor "
          f"present anywhere in {len(all_text):,} bytes")
    for rid, (hits, anch) in sorted(anchor_hits.items(),
                                    key=lambda kv: -kv[1][0])[:12]:
        print(f"     {rid:<36} {hits:>6} hits  anchors={anch}")

    ok = not (fail_recall or fail_selffp or fail_precision or fail_compile)
    print("\nRESULT:", "PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == '__main__':
    sys.exit(main())
