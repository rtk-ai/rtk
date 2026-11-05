# Security Audit — 2026-08-30

## Summary
- Issues found: 0 exploitable | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED
- All findings below are confirmed false positives (test fixtures, example placeholders, benign internal tooling, or heuristic pattern matches on documentation text). No CVE, no live secret, no code change required.

## Fixed Issues
None — no exploitable issue found.

## Unresolved Issues
None.

## False Positives Reviewed

| Tool | Finding | File(s) | Verdict |
|------|---------|---------|---------|
| Gitleaks | 38 "leaks" — stripe-access-token / generic-api-key patterns | `tests/guard_integration_test.rs`, `Security reports/security-audit-*.md` (historical), `scripts/benchmark/cloud-init.yaml`, `src/cmds/cloud/aws_cmd.rs`, `SECURITY.md` | All are placeholder/example tokens — obviously-fake sequential-digit strings (`sk` + `_live_` + repeated `1234567890abcdef`) used as fixture/doc examples, mock AWS log tokens — not real secrets |
| TruffleHog | 130 "verified" Lob-detector hits | `src/cmd/hook/mod.rs`, `src/learn/detector.rs`, `src/cmd/analysis.rs` | Rust test function names (e.g. `test_compound_quoted_operators_not_split`) coincidentally match the Lob API key regex; TruffleHog's live "verification" against Lob's API is itself a known false-positive class for this detector on identifier-shaped strings |
| Bandit | 9 Low-severity subprocess findings (B404/B603/B607) | `scripts/benchmark-sessions/lib/runner.py` | Internal benchmark tooling, not shipped/production code; fixed argument lists (`tar czf ...`), no untrusted input reaches `subprocess` |
| config-audit | HIGH "curl to external URL"; MEDIUM "skip verification" / "trust-all instruction" | `CLAUDE.md`, `claude.md` (repo root, case-variant duplicate) | Heuristic match on a documentation *example* (`curl https://api.example.com/data`) and on the repo's own "Avoiding Rabbit Holes" dev-workflow guidance — legitimate authored content, not injected instructions |
| config-audit | Remaining CRITICAL/HIGH/MEDIUM rows | Globally-installed Claude Code plugins under `~/.claude/plugins/`, `~/.claude/settings.json` | Out of scope — these are the user's global Claude Code configuration/plugin install, not part of the rtk repository; excluded from this report |

## Clean Tool Results

| Tool | Result |
|------|--------|
| OSV-Scanner | 0 vulnerabilities across `Cargo.lock` (208 packages) and test-fixture `pom.xml` files |
| Trivy (fs) | 0 vulnerabilities, 0 secrets across all scanned targets (`Cargo.lock`, fixture `pom.xml`s) |
| Semgrep — `p/secrets` | 0 findings (407 files, 45 rules) |
| Semgrep — `p/owasp-top-ten` (ts/py/rb targets) | 0 findings (12 files, 266 rules) |

## Coverage Notes
- CodeQL, mcps-audit, mcp-exfil-scan, skill-audit: not applicable — repo contains no `.github/workflows/codeql.yml`, no `mcp*.json`/`.mcp*`, and no `*.skill`/`SKILL.md` files.
- mcp-scan and skillspector LLM-mode were **not** run — both require explicit user opt-in for third-party/LLM data sharing, and this is an unattended scheduled run with no human present to consent.
- Rust source itself has no dedicated SAST tool in this pass (Semgrep's OWASP/secrets configs don't target `.rs`); `cargo clippy` (run separately in CI/pre-commit) and `cargo test --all` remain the primary Rust-specific correctness/safety gates.

## Raw Scanner Output

### Gitleaks
```
9:05PM INF 1451 commits scanned.
9:05PM INF scanned ~9999716 bytes (10 MB) in 1.06s
9:05PM WRN leaks found: 38
```
(Full 38-entry JSON detail reviewed above — all false positives.)

### OSV-Scanner
```
Scanned /Users/.../Cargo.lock file and found 208 packages
No issues found
```

### Trivy (fs)
```
Report Summary
Cargo.lock (cargo): 0 vulnerabilities
tests/fixtures/**/pom.xml (pom): 0 vulnerabilities
Legend: '0' = Clean (no security findings detected)
```

### TruffleHog
```
finished scanning {"chunks": 15702, "bytes": 11168236, "verified_secrets": 130,
"unverified_secrets": 0, "scan_duration": "5.14s"}
```
All 130 "verified" hits are Lob-detector false positives on Rust test identifiers (see table above).

### Semgrep — secrets
```
Ran 45 rules on 407 files: 0 findings.
```

### Semgrep — OWASP Top Ten
```
Ran 266 rules on 12 files: 0 findings.
Scan skipped: Not matching --include patterns: 396 | .semgrepignore: 99
```

### Bandit
```
Total issues (by severity): Low: 9, Medium: 0, High: 0
Total issues (by confidence): High: 9
All 9 in scripts/benchmark-sessions/lib/runner.py (subprocess usage, fixed args)
```

### config-audit (Claude config/skill audit)
Scanned `~/.claude/settings.json` hooks, all installed plugin `hooks.json`/`plugin.json` files, and repo `CLAUDE.md`/`claude.md`. In-repo findings limited to the two documentation false positives listed in the table above; remaining CRITICAL/HIGH/MEDIUM rows concern globally-installed plugins outside the rtk repo and are out of this audit's scope.

### APTS Audit Log
- Log: `/tmp/css-scan-20260830T130542Z.jsonl`
- Tool runs recorded: 9 (measured: 9, asserted: 0)
- Standard: OWASP APTS § Auditability
