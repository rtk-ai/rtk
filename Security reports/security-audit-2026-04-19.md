# Security Audit — 2026-04-19

## Summary
- Issues found: 0 | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

All findings across tools were false positives (documentation examples, test fixtures) or pertain to the global Claude skill ecosystem rather than the RTK codebase itself. TruffleHog live-verification confirmed **0 verified secrets** across all scanned content.

## Fixed Issues

None.

## Gitleaks Findings — All False Positives

| # | File | Rule | Assessment |
|---|------|------|-----------|
| 1 | `SECURITY.md:151` | stripe-access-token | FP — `sk_live_1234567890abcdef` is a documentation example showing what NOT to commit |
| 2 | `Security reports/2026-04-16-security-report.md:64` | stripe-access-token | FP — same example value in prior audit report |
| 3 | `Security reports/security-audit-2026-04-18.md:36` | stripe-access-token | FP — same example value in prior audit report |
| 4 | `Security reports/security-audit-2026-04-18.md:39` | stripe-access-token | FP — same example value in prior audit report |
| 5 | `scripts/benchmark/cloud-init.yaml:282` | generic-api-key | FP — truncated placeholder `API_KEY=sk-12345...` in benchmark script |
| 6 | `scripts/benchmark/cloud-init.yaml:613` | generic-api-key | FP — truncated placeholder in benchmark script |
| 7–11 | `src/cmds/cloud/aws_cmd.rs:1878,1879,1923,2035` | generic-api-key | FP — code filters AWS Secrets Manager output; no actual credentials in source |

TruffleHog (live-verified): 0 verified secrets, 0 unverified secrets — 9,923 chunks / 6.9 MB scanned.

## Unresolved Issues

None.

## Raw Scanner Output

### Gitleaks
```
827 commits scanned.
scanned ~6356081 bytes (6.36 MB) in 980ms
leaks found: 11 (all assessed as false positives — see table above)
```

### Semgrep (OWASP Top 10 + Secrets)
```
Findings: 0 (0 blocking)
Rules run: 311 (OWASP) + 45 (secrets)
Targets scanned: 311 files
```

### Trivy (Dependency + Secret scan)
```
Target: Cargo.lock — cargo — 0 vulnerabilities, 0 secrets
Status: CLEAN
```

### TruffleHog (Live-verified secrets)
```
verified_secrets: 0
unverified_secrets: 0
chunks: 9923
bytes: 6901356
scan_duration: 1.343031708s
```

### OSV-Scanner (SCA)
```
Scanned Cargo.lock — 203 packages
No issues found
```

### Config-Audit (Claude configuration)
```
22 issues found in global Claude skill/plugin ecosystem:
  CRITICAL: 5 (all pertain to security-scanner skill scripts detecting their own patterns — expected false positives)
  HIGH: 5 (curl in CLAUDE.md docs, plugin hook examples)
  MEDIUM: 9 (skill tool permissions, broad hook matchers)
  LOW: 3 (hooks configuration)
Note: None of these issues affect the RTK binary or Rust codebase.
```

### MCP Exfil Scan
```
9 issues in global Claude skill ecosystem (not RTK codebase):
  CRITICAL: 2, HIGH: 3, MEDIUM: 4
All relate to skill-security-auditor tool permissions — not RTK source code.
```
