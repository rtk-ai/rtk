# Security Audit — 2026-05-18

## Summary
- Issues found: 0 actionable | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED
- Scanners: gitleaks, trivy fs (vuln+secret+misconfig), osv-scanner, trufflehog (skipped — same coverage as gitleaks)
- Upstream commits merged: 4 (origin/develop → master)

## Findings Review

### Cargo dependencies (root Cargo.lock)
- **Trivy fs**: 0 vulnerabilities across 203 cargo packages.
- **OSV-Scanner (root Cargo.lock)**: No issues found.

### Gitleaks
12 hits — all false positives in test fixtures and historical documentation:

| # | File | Type | Classification |
|---|------|------|----------------|
| 1–4 | `src/cmds/cloud/aws_cmd.rs` lines 1878–2035 | generic-api-key | Test fixtures using fake tokens (e.g. `1234567890abcdef`) |
| 5–6 | `scripts/benchmark/cloud-init.yaml` lines 282, 613 | generic-api-key | Benchmark fixture data |
| 7 | `SECURITY.md` line 151 | stripe-access-token | Example placeholder in security documentation |
| 8–12 | `Security reports/2026-04-*.md`, `security-audit-2026-04-18.md`, `security-audit-2026-04-19.md` | stripe-access-token | Historical audit report examples |

No live credentials. No remediation required.

### Out-of-scope (worktree dependencies)
OSV-Scanner flagged `rustls-webpki 0.103.12 → 0.103.13` (RUSTSEC-2026-0104) in `.claude/worktrees/*/Cargo.lock`. These are stale worktree checkouts independent of the root project. Root Cargo.lock is unaffected.

## Fixed Issues
None — no actionable findings in scope.

## Unresolved Issues
None.

## Raw Scanner Output

### Gitleaks
```
978 commits scanned. scanned ~7.67 MB in 1.28s. leaks found: 12
generic-api-key | scripts/benchmark/cloud-init.yaml:282
generic-api-key | scripts/benchmark/cloud-init.yaml:613
generic-api-key | src/cmds/cloud/aws_cmd.rs:1878
generic-api-key | src/cmds/cloud/aws_cmd.rs:1879
generic-api-key | src/cmds/cloud/aws_cmd.rs:1923
generic-api-key | src/cmds/cloud/aws_cmd.rs:2035
stripe-access-token | SECURITY.md:151
stripe-access-token | Security reports/2026-04-16-security-report.md:64
stripe-access-token | Security reports/security-audit-2026-04-18.md:36
stripe-access-token | Security reports/security-audit-2026-04-18.md:39
stripe-access-token | Security reports/security-audit-2026-04-19.md:17
```

### Trivy fs
```
┌────────────┬───────┬─────────────────┬─────────┬───────────────────┐
│   Target   │ Type  │ Vulnerabilities │ Secrets │ Misconfigurations │
├────────────┼───────┼─────────────────┼─────────┼───────────────────┤
│ Cargo.lock │ cargo │        0        │    -    │         -         │
└────────────┴───────┴─────────────────┴─────────┴───────────────────┘
```

### OSV-Scanner (root Cargo.lock)
```
Scanned /Users/mp3wizard/Public/Claude Proxy/rtk/Cargo.lock — 203 packages
No issues found
```
