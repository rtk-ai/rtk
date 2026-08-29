//! Validation gates for `src/core/rules/secrets.toml`.
//!
//! This is the authoritative check on the ruleset. The Python tooling under
//! `scripts/secret-rules/` is the fast iteration loop, but it validates
//! patterns through a Go->Python compatibility shim; only this test compiles
//! them with the `regex` crate rtk actually ships.
//!
//! Three gates:
//!   1. recall    -- every positive matches its own rule
//!   2. self-FP   -- every negative fails to match its own rule
//!   3. precision -- no rule fires anywhere in the negative corpus of real
//!      developer-tool output
//!
//! Gate 3 is the one that matters. Redacting a commit SHA or a lockfile
//! checksum corrupts what the model reasons about with no visible symptom,
//! which is worse than the leak it would have prevented.
//!
//! The remaining tests exist because those three can quietly stop meaning
//! anything: an allowlist the regex crate rejects (and a lenient caller then
//! drops), a corpus that shrank, a `secret_group` that no longer resolves. A
//! gate that passes for the wrong reason is worse than one that fails.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use aho_corasick::AhoCorasick;
use regex::Regex;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Ruleset {
    version: u32,
    rule: Vec<Rule>,
}

#[derive(Debug, Deserialize)]
struct Rule {
    id: String,
    class: String,
    label: String,
    anchors: Vec<String>,
    pattern: String,
    entropy_min: Option<f64>,
    secret_group: Option<usize>,
    #[serde(default)]
    narrowed: bool,
    /// Upstream false-positive suppressors: known-fake values that must not be
    /// treated as secrets (AWS's documented `...EXAMPLE` keys, placeholder
    /// credentials like `curl -u user:changeme`, `{{templating}}` vars).
    /// Tested against the captured secret.
    #[serde(default)]
    allowlist_secret: Vec<String>,
    /// As above, but tested against the whole match.
    #[serde(default)]
    allowlist_match: Vec<String>,
    /// Fixtures are hex-encoded in the TOML. They are synthetic, but shaped
    /// exactly like real credentials -- which is the point -- so GitHub's push
    /// protection reads them as live secrets and blocks the push, and every
    /// fork of the repo would raise a secret-scanning alert. Upstream sidesteps
    /// this by generating samples at run time and committing almost none; we
    /// keep them committed, for reviewability, and encode them instead.
    positives_hex: Vec<String>,
    #[serde(default)]
    negatives_hex: Vec<String>,
}

/// Decode a hex fixture. Written out rather than pulling in a crate: this
/// ruleset's case for itself includes adding no new supply-chain surface, and
/// eight lines of hex decoding is not worth undermining that.
fn unhex(id: &str, s: &str) -> String {
    assert!(
        s.len().is_multiple_of(2) && s.bytes().all(|b| b.is_ascii_hexdigit()),
        "`{id}` has a malformed hex fixture: {s:?}"
    );
    let bytes: Vec<u8> = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("checked above"))
        .collect();
    String::from_utf8(bytes).unwrap_or_else(|e| panic!("`{id}` fixture is not UTF-8: {e}"))
}

impl Rule {
    fn positives(&self) -> Vec<String> {
        self.positives_hex
            .iter()
            .map(|h| unhex(&self.id, h))
            .collect()
    }

    fn negatives(&self) -> Vec<String> {
        self.negatives_hex
            .iter()
            .map(|h| unhex(&self.id, h))
            .collect()
    }
}

const RULES_TOML: &str = include_str!("../src/core/rules/secrets.toml");

fn load() -> Ruleset {
    toml::from_str(RULES_TOML).expect("secrets.toml must parse")
}

/// Shannon entropy in bits/char, matching the upstream entropy floors.
fn shannon(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let mut counts: HashMap<char, usize> = HashMap::new();
    for c in s.chars() {
        *counts.entry(c).or_insert(0) += 1;
    }
    let len = s.chars().count() as f64;
    -counts
        .values()
        .map(|&n| {
            let p = n as f64 / len;
            p * p.log2()
        })
        .sum::<f64>()
}

/// A rule with everything the regex crate needs to evaluate it: the pattern
/// and both allowlists, compiled up front so no gate has to fall back to
/// compiling — and silently skipping — them mid-scan.
struct Compiled<'a> {
    rule: &'a Rule,
    re: Regex,
    allow_secret: Vec<Regex>,
    allow_match: Vec<Regex>,
}

/// A pattern the regex crate refused, tagged with which field it came from so
/// the failure is attributable to a rule and not just to the ruleset.
struct Rejection {
    id: String,
    field: &'static str,
    pattern: String,
    error: String,
}

impl Rejection {
    fn render(&self) -> String {
        format!(
            "  {} [{}] {:?}\n      {}",
            self.id, self.field, self.pattern, self.error
        )
    }
}

impl Compiled<'_> {
    /// The substring a redactor would replace for this match: the rule's
    /// capture group, else the whole match.
    fn secret<'h>(&self, caps: &regex::Captures<'h>) -> &'h str {
        let idx = self.rule.secret_group.unwrap_or(1);
        caps.get(idx)
            .or_else(|| caps.get(0))
            .map(|m| m.as_str())
            .unwrap_or("")
    }

    /// Every match in `hay` that would actually be redacted.
    ///
    /// Each match is judged on its own: a hit below the entropy floor, or one
    /// an allowlist suppresses, says nothing about the next one. Scanning only
    /// the first match would let a low-entropy hit at the top of a fixture
    /// vouch for the several hundred lines beneath it — 89 of the rules carry
    /// an entropy floor, so that is not a hypothetical.
    fn firing<'h>(&self, hay: &'h str) -> Vec<&'h str> {
        self.re
            .captures_iter(hay)
            .filter_map(|caps| {
                let whole = caps.get(0)?.as_str();
                let secret = self.secret(&caps);
                if self
                    .rule
                    .entropy_min
                    .is_some_and(|min| shannon(secret) < min)
                {
                    return None;
                }
                if self.allow_secret.iter().any(|a| a.is_match(secret))
                    || self.allow_match.iter().any(|a| a.is_match(whole))
                {
                    return None;
                }
                Some(secret)
            })
            .collect()
    }

    fn fires(&self, hay: &str) -> bool {
        !self.firing(hay).is_empty()
    }
}

/// Compile one allowlist, pushing any rejection onto `bad` rather than
/// dropping it. `Regex::new(p).ok()` here is what hid a live false positive.
fn compile_list(
    id: &str,
    field: &'static str,
    pats: &[String],
    bad: &mut Vec<Rejection>,
) -> Vec<Regex> {
    let mut out = Vec::new();
    for p in pats {
        match Regex::new(p) {
            Ok(re) => out.push(re),
            Err(e) => bad.push(Rejection {
                id: id.to_string(),
                field,
                pattern: p.clone(),
                error: e.to_string(),
            }),
        }
    }
    out
}

/// Compile every rule, collecting rejections rather than panicking on the
/// first. One run should report every offender.
///
/// A rule whose *pattern* is rejected is dropped; a rule whose *allowlist* is
/// rejected is kept without that suppressor, which is the strict direction —
/// the missing suppressor can only produce extra hits, and the dedicated gate
/// below names the offender either way.
fn compile_each(rs: &Ruleset) -> (Vec<Compiled<'_>>, Vec<Rejection>) {
    let mut ok = Vec::new();
    let mut bad = Vec::new();

    for r in &rs.rule {
        let allow_secret = compile_list(&r.id, "allowlist_secret", &r.allowlist_secret, &mut bad);
        let allow_match = compile_list(&r.id, "allowlist_match", &r.allowlist_match, &mut bad);
        match Regex::new(&r.pattern) {
            Ok(re) => ok.push(Compiled {
                rule: r,
                re,
                allow_secret,
                allow_match,
            }),
            Err(e) => bad.push(Rejection {
                id: r.id.clone(),
                field: "pattern",
                pattern: r.pattern.clone(),
                error: e.to_string(),
            }),
        }
    }
    (ok, bad)
}

/// Rules that compile. The dedicated compile tests are what fail on rejects,
/// so the behavioural gates below run against everything that did build.
fn compile_all(rs: &Ruleset) -> Vec<Compiled<'_>> {
    compile_each(rs).0
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/secrets/negative")
}

/// Floor on the negative corpus, ~12% under the ~68KB it holds today. The
/// precision gate is only as good as the output it runs against, and a corpus
/// that quietly shrinks — a deleted fixture, a truncated capture — weakens it
/// without failing anything.
const CORPUS_MIN_BYTES: usize = 60_000;

fn corpus() -> Vec<(String, String)> {
    let dir = fixture_dir();
    let mut out: Vec<_> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("negative corpus missing at {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "txt"))
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            // Not `unwrap_or_default`: an unreadable or non-UTF-8 fixture would
            // become an empty string and silently excuse every rule from being
            // tested against it.
            let body = fs::read_to_string(e.path())
                .unwrap_or_else(|err| panic!("corpus fixture {name} unreadable: {err}"));
            (name, body)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(!out.is_empty(), "negative corpus is empty");

    let total: usize = out.iter().map(|(_, t)| t.len()).sum();
    assert!(
        total >= CORPUS_MIN_BYTES,
        "negative corpus shrank to {total} bytes across {} files (floor {CORPUS_MIN_BYTES}); \
         the precision gate is only as strong as this corpus",
        out.len()
    );
    out
}

// ---------------------------------------------------------------- structure

#[test]
fn ruleset_is_well_formed() {
    let rs = load();
    assert_eq!(rs.version, 1);
    assert!(
        rs.rule.len() >= 100,
        "expected the full set, got {}",
        rs.rule.len()
    );

    let mut seen = std::collections::HashSet::new();
    for r in &rs.rule {
        assert!(seen.insert(&r.id), "duplicate rule id `{}`", r.id);
        assert!(!r.label.is_empty(), "`{}` has no label", r.id);
        assert!(
            matches!(r.class.as_str(), "A" | "B"),
            "`{}` has unknown class `{}`",
            r.id,
            r.class
        );
        assert!(!r.positives_hex.is_empty(), "`{}` ships no positive", r.id);
        assert!(!r.anchors.is_empty(), "`{}` has no anchor", r.id);
        for a in &r.anchors {
            assert_eq!(a, &a.to_lowercase(), "anchor `{a}` must be lowercased");
            let usable = a.len() >= 3 || (a.len() == 2 && !a.chars().all(|c| c.is_alphanumeric()));
            assert!(
                usable,
                "`{}` anchor `{a}` is too weak for the prefilter",
                r.id
            );
        }
    }
}

/// The whole ruleset must compile under the `regex` crate. Nine patterns carry
/// mid-expression `(?i)`, which Go and Rust accept but Python does not -- the
/// Python tooling hoists them, so this is the only place they are checked as
/// written.
///
/// A rejection here is not only a build error -- the regex crate's default
/// 10MB program limit is a proxy for how much memory the pattern costs at
/// runtime. rtk compiles the whole set, so an expensive pattern is a budget
/// problem, not just a compile problem. Fix the pattern; do not raise the
/// limit.
#[test]
fn every_pattern_compiles_with_the_regex_crate() {
    let rs = load();
    let (ok, bad) = compile_each(&rs);
    let bad: Vec<_> = bad.iter().filter(|r| r.field == "pattern").collect();
    assert!(
        bad.is_empty(),
        "{} of {} patterns rejected by the regex crate:\n{}",
        bad.len(),
        rs.rule.len(),
        bad.iter()
            .map(|r| r.render())
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert_eq!(ok.len(), rs.rule.len());
}

/// Allowlists cross the same Go->Rust boundary as the patterns, and they fail
/// in the worse direction: a rejected allowlist is a suppressor that silently
/// stops suppressing, so the rule starts redacting the very placeholders the
/// allowlist was imported to protect.
///
/// This gate exists because that happened. `curl-auth-user` carried upstream's
/// `{{templating}}` suppressor as `['"]?\$?{{[^}]+}}...`, which Go and Python
/// both read as literal braces and the regex crate rejects outright
/// ("repetition quantifier expects a valid decimal"). Nothing noticed: the
/// Python loop compiled it happily, and the Rust side dropped it with
/// `Regex::new(p).ok()`. `curl -u "${{ secrets.USER }}:${{ secrets.TOKEN }}"`
/// — an ordinary CI log line — was a live false positive the whole time.
#[test]
fn every_allowlist_compiles_with_the_regex_crate() {
    let rs = load();
    let (_, bad) = compile_each(&rs);
    let bad: Vec<_> = bad.iter().filter(|r| r.field != "pattern").collect();
    assert!(
        bad.is_empty(),
        "{} allowlist pattern(s) rejected by the regex crate -- these fail \
         OPEN, in the direction that redacts:\n{}",
        bad.len(),
        bad.iter()
            .map(|r| r.render())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// `secret_group` names the capture a redactor replaces. Point it past the end
/// and `caps.get(idx)` yields `None`, the lookup falls back to group 0, and the
/// rule quietly widens from redacting a token to redacting its whole match —
/// a one-character typo with no symptom.
///
/// Only the explicit index is checked. A rule with no capture group at all is
/// legitimate and common: 48 of them, where the match *is* the token
/// (`glpat-<20>`, a whole PEM block), so group 0 is the right target.
/// `class_a_capture_spans_the_whole_secret` is what holds that line.
#[test]
fn secret_group_indexes_an_existing_capture_group() {
    let rs = load();
    let mut bad = Vec::new();
    for c in compile_all(&rs) {
        let groups = c.re.captures_len(); // includes group 0
        if let Some(idx) = c.rule.secret_group {
            if idx >= groups {
                bad.push(format!(
                    "  {} sets secret_group = {idx} but the pattern has {} capture group(s)",
                    c.rule.id,
                    groups - 1
                ));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "secret_group does not resolve -- would silently fall back to the \
         whole match:\n{}",
        bad.join("\n")
    );
}

// -------------------------------------------------------------- gate 1 & 2

#[test]
fn gate_1_recall_every_positive_matches() {
    let rs = load();
    let mut failures = Vec::new();
    for c in compile_all(&rs) {
        for p in &c.rule.positives() {
            if !c.fires(p) {
                failures.push(format!("  {} did not match its positive: {p:?}", c.rule.id));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "recall failures:\n{}",
        failures.join("\n")
    );
}

#[test]
fn gate_2_no_rule_matches_its_own_negatives() {
    let rs = load();
    let mut failures = Vec::new();
    for c in compile_all(&rs) {
        for n in &c.rule.negatives() {
            if c.fires(n) {
                failures.push(format!("  {} wrongly matched: {n:?}", c.rule.id));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "self-FP failures:\n{}",
        failures.join("\n")
    );
}

// ------------------------------------------------------------------ gate 3

#[test]
fn gate_3_precision_no_rule_fires_on_real_output() {
    let rs = load();
    let compiled = compile_all(&rs);
    let mut failures = Vec::new();

    for (name, text) in corpus() {
        for c in &compiled {
            let hits = c.firing(&text);
            if hits.is_empty() {
                continue;
            }
            // Report the count, not just the first: "fired once" and "fired 400
            // times" are very different amounts of corrupted output.
            failures.push(format!(
                "  {} fired {} time(s) in {name}, first on {:?}",
                c.rule.id,
                hits.len(),
                hits[0].chars().take(60).collect::<String>()
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "precision failures -- these would corrupt agent input:\n{}",
        failures.join("\n")
    );
}

// --------------------------------------------------------- memory budget

/// Per-rule ceiling on compiled program size. Far below the regex crate's
/// 10MB default, because rtk holds the whole set in memory and a single
/// expensive rule is invisible until you measure it.
const PER_RULE_LIMIT: usize = 2 * 1024 * 1024;

/// Guards the most expensive mistake found while building this set.
///
/// `\w`, `\d` and `\s` are Unicode-aware in the regex crate and compile to
/// large UTF-8 automata. Inside a bounded repeat the cost multiplies: upstream's
/// `[\w-]{138,300}` for a Vault batch token compiled to 14.3MB and pypi's
/// `[\w-]{50,1000}` to 47.7MB -- both past the 10MB ceiling, and the whole
/// ruleset came to 122MB. Rewritten with explicit ASCII classes the same
/// patterns are 38KB and 140KB, and the set totals 2.7MB.
///
/// Every token here is ASCII, so the rewrite is behaviour-preserving -- the
/// recall gate proves the positives still match. This test stops the Unicode
/// forms creeping back in via a regeneration.
#[test]
fn no_rule_exceeds_its_memory_budget() {
    let rs = load();
    let mut over = Vec::new();
    for r in &rs.rule {
        let built = regex::RegexBuilder::new(&r.pattern)
            .size_limit(PER_RULE_LIMIT)
            .build();
        if built.is_err() {
            over.push(format!(
                "  {} exceeds {}KB compiled -- replace \\w/\\d/\\s with \
                 explicit ASCII classes",
                r.id,
                PER_RULE_LIMIT / 1024
            ));
        }
    }
    assert!(
        over.is_empty(),
        "{} rule(s) over the per-rule memory budget:\n{}",
        over.len(),
        over.join("\n")
    );
}

/// The Unicode shorthands are what blew the budget; explicit classes are the
/// fix. Catch a regression at the source rather than waiting for the size
/// symptom, which only shows up on rules with large repeats.
#[test]
fn patterns_use_ascii_classes_not_unicode_shorthands() {
    let rs = load();
    let mut offenders = Vec::new();
    for r in &rs.rule {
        // A literal backslash-w/d/s, not an escaped backslash before one.
        let mut chars = r.pattern.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if c != '\\' {
                continue;
            }
            match r.pattern[i + 1..].chars().next() {
                Some('w') | Some('d') | Some('s') => {
                    offenders.push(format!(
                        "  {} uses \\{}",
                        r.id,
                        r.pattern[i + 1..].chars().next().unwrap()
                    ));
                    break;
                }
                Some('\\') => {
                    chars.next();
                }
                _ => {}
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "Unicode-aware shorthands found ({} rule(s)); regenerate with \
         scripts/secret-rules/build.py:\n{}",
        offenders.len(),
        offenders.join("\n")
    );
}

// ------------------------------------------------------------- prefilter

/// The design assumes an aho-corasick pass over anchors rejects almost
/// everything before any regex runs. Verify that on real output: if this ratio
/// collapses, the <10ms budget goes with it.
#[test]
fn anchor_prefilter_rejects_almost_every_rule() {
    let rs = load();
    let all: String = corpus()
        .iter()
        .map(|(_, t)| t.as_str())
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();

    let mut with_hits = 0usize;
    for r in &rs.rule {
        let ac = AhoCorasick::new(&r.anchors).expect("anchors build");
        if ac.find(&all).is_some() {
            with_hits += 1;
        }
    }

    let total = rs.rule.len();
    let pct = 100.0 * with_hits as f64 / total as f64;
    println!(
        "anchors present for {with_hits}/{total} rules ({pct:.1}%) over {} bytes",
        all.len()
    );
    assert!(
        with_hits * 5 <= total,
        "prefilter degraded: {with_hits}/{total} rules would run their regex \
         on ordinary output; expected under 20%"
    );
}

/// A redactor replaces the *captured group*, so for a Class A rule -- a
/// self-contained token -- that group must span essentially the whole match.
/// Two upstream patterns capture an internal fragment instead (a repeated UUID
/// chunk, a named `alg` segment); redacting those would leave most of the
/// secret on screen. gitleaks never had to care: locating a secret and
/// replacing one are different jobs.
///
/// Class B rules are exempt. They deliberately capture only the value so the
/// surrounding structure (`kind: Secret`, the `<add key=...>` element) survives
/// and the output still reads like the real command's.
#[test]
fn class_a_capture_spans_the_whole_secret() {
    let rs = load();
    let mut thin = Vec::new();
    for c in compile_all(&rs) {
        if c.rule.class != "A" {
            continue;
        }
        // `first()`, not `[0]`: the structure gate owns "every rule ships a
        // positive", and it should report that as an assertion, not as an
        // index panic here.
        let positives = c.rule.positives();
        let Some(probe) = positives.first() else {
            continue;
        };
        let Some(caps) = c.re.captures(probe) else {
            continue;
        };
        let full = caps.get(0).map(|m| m.as_str().len()).unwrap_or(0);
        let grp = c.secret(&caps).len();
        if full > 0 && (grp as f64 / full as f64) < 0.5 {
            thin.push(format!(
                "  {} captures {grp}/{full} chars -- set secret_group",
                c.rule.id
            ));
        }
    }
    assert!(
        thin.is_empty(),
        "class A rules whose capture would leave the secret partly visible:\n{}",
        thin.join("\n")
    );
}

/// Fixtures must stay encoded. A regeneration that emitted them in plaintext
/// would look fine locally and then be rejected at the remote by push
/// protection -- or, worse, accepted and left raising a secret-scanning alert
/// in every fork. Checking the raw file is the only way to see it: by the time
/// a fixture reaches the other tests it has already been decoded.
#[test]
fn fixtures_are_not_committed_in_plaintext() {
    let rs = load();
    for r in &rs.rule {
        for h in r.positives_hex.iter().chain(r.negatives_hex.iter()) {
            assert!(
                h.bytes().all(|b| b.is_ascii_hexdigit()),
                "`{}` has a fixture that is not hex -- regenerate with \
                 scripts/secret-rules/build.py",
                r.id
            );
        }
    }
    // The plaintext field names must not reappear alongside the encoded ones.
    for field in ["\npositives = [", "\nnegatives = ["] {
        assert!(
            !RULES_TOML.contains(field),
            "secrets.toml contains a plaintext `{}` array",
            field.trim()
        );
    }
}

/// Narrowed patterns are a deliberate divergence from upstream and must stay
/// documented, so the next person regenerating the set does not silently
/// revert them.
#[test]
fn narrowed_rules_are_the_expected_ones() {
    let rs = load();
    let mut narrowed: Vec<&str> = rs
        .rule
        .iter()
        .filter(|r| r.narrowed)
        .map(|r| r.id.as_str())
        .collect();
    narrowed.sort_unstable();
    assert_eq!(
        narrowed,
        ["sourcegraph-access-token", "vault-service-token"],
        "narrowed set changed -- update src/core/rules/README.md too"
    );
}
