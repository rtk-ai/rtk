# Security Audit — 2026-08-07

## Summary
- Issues found: 0 | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED
- Scope: full rtk source tree (`src/`, `Cargo.toml`, `Cargo.lock`, `tests/`, docs), git history included.
- Tools run: Gitleaks 8.30.1, Trivy 0.72.0, TruffleHog 3.95.9, OSV-Scanner 2.4.0, Semgrep (p/secrets ruleset). Bandit skipped (no Python files).

## Fixed Issues
None — no real vulnerabilities detected.

## Unresolved Issues
None.

## False Positives (reviewed, no action needed)
| Tool | Count | Cause |
|------|-------|-------|
| Gitleaks | 36 | Placeholder tokens (`[REDACTED-stripe-access-token]`, `[REDACTED-generic-api-key]`) in past `Security reports/*.md` audit docs and `scripts/benchmark/cloud-init.yaml` / `src/cmds/cloud/aws_cmd.rs` test fixtures — canonical example values, not live credentials. |
| TruffleHog | 115 | "Lob" detector (Lob.com API key format) false-matching plain Rust test-function names (e.g. `test_filter_removes_trailing_empty_lines`) as verified secrets. No actual API key material. |
| Trivy | 0 | — |
| OSV-Scanner | 0 | — |
| Semgrep (secrets) | 0 | — |

## Raw Scanner Output

### Gitleaks
```
9:16AM INF 1318 commits scanned.
9:16AM INF scanned ~9047546 bytes (9.05 MB) in 2.17s
9:16AM WRN leaks found: 36
```
36/36 findings reviewed individually via JSON report — all are `stripe-access-token` / `generic-api-key` rule matches on documentation placeholder values (redacted above) or fixture data. No live secret material present.

### Trivy (`trivy fs`)
Scanned all `Cargo.lock` (202 packages) and `pom.xml` fixture files across the working tree and worktrees. Result: **0 vulnerabilities** in every scanned lockfile.
```
Legend:
- '-': Not scanned
- '0': Clean (no security findings detected)
```

### TruffleHog (`trufflehog git`)
```
finished scanning {"chunks": 14089, "bytes": 10103224, "verified_secrets": 115, "unverified_secrets": 0, "scan_duration": "1.483075583s", ...}
```
All 115 "verified" results are `Detector Type: Lob` false positives on test function names in `src/tree.rs` and `src/learn/detector.rs`. No credential-shaped secrets found.

### OSV-Scanner (`osv-scanner scan source -r`)
```
Scanned Cargo.lock file and found 202 packages (x5 worktrees + main)
Scanned pom.xml fixture files (0-1 packages each)
No issues found
```

### Semgrep (`p/secrets`)
```
Scanning 398 files tracked by git with 52 Code rules
Findings: 0 (0 blocking)
Rules run: 45
Targets scanned: 398
```

### APTS Audit Log
- **Log:** `/tmp/css-scan-20260807T021012Z.jsonl`
- **Tool runs recorded:** 6 (measured: 6, asserted: 0)
- **Standard:** OWASP APTS § Auditability
