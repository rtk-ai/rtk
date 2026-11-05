# Security Audit — 2026-10-01

## Summary
- Issues found: 1 category (38 raw findings) | Auto-fixed: 0 | Unresolved: 0 (all triaged as false positive, no code change required)
- Status: PASSED

## Fixed Issues
None — no true vulnerabilities requiring a code or dependency change were found.

## Triaged Findings (no fix required)

| # | Component | Tool | Finding | Disposition |
|---|-----------|------|---------|--------------|
| 1 | `Security reports/security-audit-*.md` (27 hits), `Security reports/2026-04-16-security-report.md`, `SECURITY.md` | Gitleaks | `stripe-access-token` / `generic-api-key` pattern matches | **False positive.** Prior audit reports and `SECURITY.md` quoting example/placeholder strings that match Stripe/generic key regexes; not live credentials. Recurring since at least 2026-08-12's audit. |
| 2 | `tests/guard_integration_test.rs:166,184` | Gitleaks | `stripe-access-token` pattern match | **False positive.** Fixture literal `sk_live_FAKE...` (explicitly labeled FAKE in source), used to test rtk's secret-guard filter. |
| 3 | `src/cmds/cloud/aws_cmd.rs:1878-2035` | Gitleaks | `generic-api-key` pattern matches | **False positive.** Matches are inside `#[cfg(test)]` unit tests — synthetic AWS CloudWatch Logs pagination tokens (nextForwardToken/nextBackwardToken, a hex placeholder string) used as filter-output fixtures. |
| 4 | `scripts/benchmark/cloud-init.yaml:282,613` | Gitleaks | `generic-api-key` pattern match | **False positive.** Cloud-init benchmark fixture with placeholder key string, not a live credential. |

TruffleHog (verified-secrets mode) independently confirms: **0 verified secrets** across the same git history — corroborating that none of the 38 Gitleaks hits are real.

## Clean Scanners
- **TruffleHog** (git history, `--only-verified`): 0 verified secrets.
- **OSV-Scanner** (`Cargo.lock`, 213 packages): No issues found.
- **Trivy** (filesystem — vuln/secret/misconfig, excl. `target/` and nested worktrees): 0 vulnerabilities, 0 secrets, 0 misconfigs across `Cargo.lock` and all `pom.xml`/kubernetes fixtures. Trivy version 0.74.0 — not in the compromised 0.69.4-0.69.6 range.
- **Semgrep** (`p/secrets`, `src/` + `Cargo.toml`, 228 files): 0 findings.
- **Bandit**: Skipped — no `.py` files inside the audit scope (`src/`, `Cargo.toml`, `Cargo.lock`); a handful of `.py` files exist only under unrelated sibling worktrees, out of this task's stated scope.
- **mcps-audit / skill-audit / mcp-exfil-scan / CodeQL**: Skipped — out of this task's stated scope (Rust source, `Cargo.toml`, `Cargo.lock` only); no MCP/skill files are part of the audited path.

## Unresolved Issues
None.

## Raw Scanner Output

### Pre-flight
```
OK  bandit bandit 1.9.4
OK  semgrep 1.177.0
OK  trivy Version: 0.74.0
OK  trufflehog trufflehog 3.97.5
OK  gitleaks gitleaks version 8.30.1
OK  osv-scanner osv-scanner version: 2.6.0
OK  jq
```

### Gitleaks
```
gitleaks detect --source <target> --no-banner
1912 commits scanned, ~13.84 MB scanned in 765ms
leaks found: 38 (all triaged as false positive — see table above)
```

### TruffleHog
```
trufflehog git file://<target> --no-update --only-verified
chunks: 21232, bytes: 15475699
verified_secrets: 0
unverified_secrets: 0
scan_duration: 1.065s
```

### OSV-Scanner
```
osv-scanner scan -L <target>/Cargo.lock
Scanned Cargo.lock: 213 packages
No issues found
```

### Trivy
```
trivy fs <target> --skip-dirs .claude/worktrees --skip-dirs target --scanners vuln,secret,misconfig
Cargo.lock (cargo): 0 vulnerabilities
tests/fixtures/multi-module-{fail-,}skeleton/**/pom.xml x6 (pom): 0 vulnerabilities each
tests/fixtures/oc_pods.json (kubernetes): 0 misconfigurations
Secret scanning: 0 findings
```

### Semgrep
```
semgrep scan --config p/secrets <target>/src <target>/Cargo.toml
Scanning 228 files tracked by git with 52 Code rules + 36 multilang rules
Findings: 0 (0 blocking)
Scan skipped: 1 file larger than 0.3 MB
```

### APTS Audit Log
- **Log:** `/tmp/css-scan-20261001T021454Z.jsonl`
- **Tool runs recorded:** 5 (measured: 5, asserted: 0)
- **Standard:** OWASP APTS § Auditability
