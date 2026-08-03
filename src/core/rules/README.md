# Secret-detection ruleset

`secrets.toml` is the data behind rtk's secret redaction. It is **data only** —
no engine lives here, so it can be consumed by any redactor implementation
without picking a side on module layout.

Regenerate with:

```bash
python3 scripts/secret-rules/build.py     # fetches pinned gitleaks, rebuilds secrets.toml
cargo test --test secret_rules            # authoritative validation
```

## Why not just import gitleaks wholesale?

gitleaks scans **repositories**. rtk scans **command output**. The distinction
is not cosmetic:

- A 40-char hex blob in a file near the word "sourcegraph" is a plausible
  token. The same blob in `git log` output is a commit SHA.
- A vendor name (`twitter`, `airtable`) is a useful signal in source, where it
  appears as `twitter_api_key = "..."`. rtk never sees assignments — it sees
  the stdout of `kubectl`, `curl`, `git`, `env`.

And the cost of a mistake is inverted. gitleaks failing open annoys a
developer. rtk failing *closed* — redacting a commit SHA — silently corrupts
what the model reasons about downstream, with no visible symptom. **Precision is
the hard constraint; recall is negotiable.**

## Inclusion criteria

222 upstream rules → 103 kept. A rule ships only if all hold:

1. **Class A or B.** Class A is a self-identifying anchor (`ghp_`, `AKIA`,
   `sk-ant-api03`). Class B is a structural shape (PEM block, JWT, `curl -H
   "Authorization: …"`, a k8s Secret manifest). Rules whose only keyword is the
   vendor's own name are rejected — but see the note below, because the reason
   is not the obvious one.
2. **Plausibly appears in developer CLI output** — cloud, VCS/CI, registries,
   AI providers, secret managers, observability, dev SaaS. The consumer and
   fintech long tail is dropped.
3. **At least one usable anchor** — ≥3 chars, or 2 chars including
   punctuation. Anchors feed an aho-corasick prefilter; short ones fire
   constantly on prose and would blow rtk's <10ms budget.
4. **At least one positive fixture.** A rule that cannot demonstrate a match
   does not ship.

### Why vendor-name rules are dropped

Not because they cannot fire on command output — they can. A differential run
against upstream gitleaks showed 3 of 6 sampled vendor-name rules matching
env-style lines, which `rtk env`, `docker inspect` and `systemctl show` all
produce:

```
HEROKU_API_KEY=01234567-89ab-cdef-0123-456789abcdef    matches upstream rule
OKTA_API_TOKEN=00aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789 matches upstream rule
SNYK_TOKEN=12345678-1234-1234-1234-123456789012        matches upstream rule
```

They are dropped because **70 per-vendor rules are the wrong shape for this
job**. A `KEY=VALUE` line is identified by the *key name*, not the vendor, so
~40 generic key-name rules (`*_KEY`, `*_TOKEN`, `*PASSWORD*`, `*SECRET*`) cover
every vendor that exists and every one shipped next year — at a fraction of the
anchor cost. Note that generic rules would also catch the three cases upstream's
own vendor rules *miss* (`TWITTER_API_KEY`, `DATADOG_API_KEY`,
`CLOUDFLARE_API_KEY` in the same sample).

That generic key-name class is not implemented yet, so **there is a real
coverage gap here today**. It is a deliberate deferral, not an oversight.

## Allowlists

Six rules carry `allowlist_secret` / `allowlist_match` imported from upstream.
These suppress known-fake values — AWS's documented `...EXAMPLE` keys,
placeholder credentials (`curl -u user:changeme`), `{{templating}}` variables.
They are upstream's false-positive tuning; dropping them would ship a noisier
ruleset than the one we imported. Path-based allowlists are skipped: they gate
on a file's location, and a stream has none.

## Narrowed patterns

Two rules are deliberately narrower than upstream. Both are marked
`narrowed = true` and justified inline in `scripts/secret-rules/build.py`:

| rule | change | reason |
|---|---|---|
| `sourcegraph-access-token` | dropped the bare `[a-fA-F0-9]{40}` branch | matched every git commit SHA — caught by the corpus gate |
| `vault-service-token` | dropped the legacy `s.<24>` form | the `s.` anchor fires ~350×/250KB ("as.", "is.", "this."); Vault no longer issues that format |

## ASCII character classes

Patterns are rewritten to use explicit ASCII classes (`[0-9A-Za-z_]`) instead of
`\w`, `\d` and `\s`. In the `regex` crate those shorthands are Unicode-aware and
compile to large UTF-8 automata; inside a bounded repeat the cost multiplies:

| rule | upstream `\w` | ASCII |
|---|---|---|
| `pypi-upload-token` `{50,1000}` | 47.7 MB | **139.5 KB** |
| `vault-batch-token` `{138,300}` | 14.3 MB | **38.1 KB** |
| **whole ruleset** | **122.5 MB** | **2.7 MB** |

Both oversized rules were outright *rejected* by the regex crate's 10MB
ceiling — so this was a hard build failure, not just a tuning matter. The
Python tooling never saw it: Python's backtracking engine compiles those
patterns happily. It only surfaced under `cargo test`.

Every token these rules match is ASCII (base64/base64url/hex), so the rewrite
is behaviour-preserving — the recall gate proves the positives still match.
Two tests keep it that way: `no_rule_exceeds_its_memory_budget` (2MB per rule)
and `patterns_use_ascii_classes_not_unicode_shorthands`.

Measure the current cost with `cargo run --release --example regex_probe`.

## Literal braces

Upstream patterns are RE2, and RE2 is *not* a subset of what the `regex` crate
accepts. A bare `{` that cannot begin a repetition is a literal in Go and in
Python; the regex crate rejects the whole pattern with "repetition quantifier
expects a valid decimal". So `{{templating}}` suppressors have to be written
`\{\{…\}\}`. `escape_literal_braces()` in `build_rules.py` does this to patterns
and allowlists alike, leaving real quantifiers (`{93}`, `{138,300}`) and braces
inside a character class untouched.

This is not academic. `curl-auth-user` shipped upstream's suppressor verbatim,
the regex crate rejected it, and the rejection was discarded by
`Regex::new(p).ok()` — so an ordinary CI line,

```
curl -u "${{ secrets.REGISTRY_USER }}:${{ secrets.REGISTRY_TOKEN }}" …
```

matched and would have been redacted as a live credential. An allowlist that
fails to compile fails **open**, in the direction that redacts, which is why
`every_allowlist_compiles_with_the_regex_crate` is a gate and not a lint.

## Why fixtures are hex-encoded

`positives_hex` / `negatives_hex` hold the fixtures encoded, with a truncated
plaintext preview above each:

```toml
positives_hex = [
  # sk-ant-api03... (108 chars)
  "736b2d616e742d61706930332d...",
]
```

The fixtures are synthetic, but they are shaped exactly like real credentials —
that is the entire point of them. GitHub's push protection therefore reads them
as live secrets and refuses the push (44 detections on the first attempt), and
committing them plaintext would raise a secret-scanning alert in every fork of
the repo.

Upstream avoids this by generating samples at run time and committing almost
none — only 18 of 222 gitleaks rules ship a literal true-positive. We keep
fixtures committed, because a reviewer can check a fixture and cannot check a
generator, and encode them instead. Hex rather than base64: some scanners decode
base64, none decode hex, and the only cost is file size.

`fixtures_are_not_committed_in_plaintext` gates it. `tests/secret_rules.rs`
decodes with a local helper rather than a crate, so this adds no dependency.

## Bound coverage

A fixture is generated once, at the *minimum* of each `{lo,hi}`. That leaves the
top of every range untested — an upper limit far below what a vendor actually
issues still reads as passing, because nothing ever asks for a longer token.
**39 of the 103 rules carry a variable bound**; the other 64 are a single fixed
length, where one sample is already exhaustive.

So `build.py` generates each variable-bound rule at min, mid and max and asserts
all three match — **105 variants** today. Only one is committed, so this costs
nothing in fixtures or repo hygiene. Four rules are skipped and counted, not
silently passed: they are the contextual ones the sampler cannot build at any
length, which is why they carry hand-written fixtures.

## Capture groups

A redactor replaces the **captured group**, not the whole match. gitleaks only
ever had to *locate* a secret, so two upstream patterns capture an internal
fragment — `microsoft-teams-webhook` captures a 5-char UUID chunk out of a
198-char webhook URL, `jwt-base64` captures the named `alg` segment. Redacting
those would leave most of the secret on screen. Both carry `secret_group = 0`.

Class B rules capture only the *value* on purpose (`password: <b64>`, not the
whole manifest) so `kind: Secret` stays visible and the output still reads like
the real command's. `class_a_capture_spans_the_whole_secret` enforces the
distinction.

## Schema

| field | meaning |
|---|---|
| `id` | upstream rule id, kept for traceability |
| `class` | `A` self-identifying prefix, `B` structural shape |
| `label` | appears in the placeholder: `[REDACTED:<label>:<hash>]` |
| `anchors` | lowercased literals for the aho-corasick prefilter |
| `pattern` | lookaround/backref free, and adjusted where RE2 and the `regex` crate disagree — see *ASCII character classes* and *Literal braces* |
| `entropy_min` | Shannon floor on the captured group, when upstream sets one |
| `secret_group` | capture group holding the secret, when not group 1 |
| `narrowed` | present when the pattern differs from upstream |
| `positives` | must match |
| `negatives` | must not match |

## Testing

`tests/secret_rules.rs` enforces three gates:

1. **Recall** — every positive matches its own rule.
2. **Self-FP** — every negative fails to match its own rule.
3. **Precision** — no rule fires anywhere in
   `tests/fixtures/secrets/negative/`, ~68KB of real output: `Cargo.lock`
   checksums, git SHAs, `git log`/`git diff`, container digests, npm integrity
   hashes, UUIDs, a base64 blob, rtk's own source and README, and English
   prose.

Gate 3 is the one that matters. When adding a rule, add to the corpus too —
a rule that has never been tested against real output has not been tested. It
scans *every* match in a fixture, not just the first: 89 of the 103 rules carry
an entropy floor, so a single low-entropy hit at the top of a file must not be
allowed to vouch for the rest of it.

The remaining tests guard the ways those three can pass without meaning
anything:

| test | guards |
|---|---|
| `every_pattern_compiles_with_the_regex_crate` | a pattern RE2 accepts and `regex` rejects |
| `every_allowlist_compiles_with_the_regex_crate` | a suppressor that silently stops suppressing |
| `secret_group_indexes_an_existing_capture_group` | an index past the end, which falls back to group 0 and widens redaction |
| `no_rule_exceeds_its_memory_budget` | 2MB compiled per rule |
| `patterns_use_ascii_classes_not_unicode_shorthands` | the Unicode forms creeping back via a regeneration |
| `anchor_prefilter_rejects_almost_every_rule` | the prefilter that keeps rtk under 10ms |
| `class_a_capture_spans_the_whole_secret` | a capture that would leave the secret partly on screen |
| `narrowed_rules_are_the_expected_ones` | a divergence from upstream silently reverting |
| `ruleset_is_well_formed` | ids, labels, classes, anchor strength |

A note on the corpus floor: `corpus()` asserts at least 60KB across the fixture
directory and panics on an unreadable file rather than treating it as empty.
Both failure modes look exactly like a clean run otherwise.

On the committed fixture corpus only **2 of 103** rules have an anchor present
anywhere, so 101 never run their regex at all. `validate.py` reports a higher
count because it builds a larger corpus live from the working tree (~290KB of
`git log`/`git diff`/`Cargo.lock`); that is the same measurement over more
text, not a different one.

## Provenance

Patterns and false-positive cases derive from
[gitleaks](https://github.com/gitleaks/gitleaks) (MIT), pinned to **v8.30.1**
(`83d9cd684c87d95d656c1458ef04895a7f1cbd8e`). See `NOTICE`. rtk is Apache-2.0;
MIT is compatible. TruffleHog is **AGPL-3.0** and must not be used as a source.

The pin is a commit SHA, not a branch, and `build.py` checksums the config file
it downloads. `master` meant two regenerations a month apart could produce
different rulesets from the same command — awkward for a derived work whose
NOTICE has to name what it derives from. To bump it, edit `GITLEAKS_REF`, run
`build.py --update-pin` for the new digest, then re-run the gates.
