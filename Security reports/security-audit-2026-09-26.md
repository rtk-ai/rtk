# Security Audit — 2026-09-26

## Summary
- Issues found: 0 | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

Scope: full rtk codebase after merging 43 upstream commits (`origin/develop`). Tools run: osv-scanner (Cargo.lock, 213 packages), trivy fs (vuln+secret), gitleaks (filesystem), semgrep `p/rust` on `src`.

## Raw Scanner Output
- osv-scanner: No issues found (213 packages).
- trivy fs: 0 vulnerabilities in Cargo.lock and test-fixture pom.xml files; 0 secrets.
- gitleaks: 36 hits, all known false positives (stripe/generic-api-key patterns in prior audit reports, SECURITY.md, `tests/guard_integration_test.rs`, `src/cmds/cloud/aws_cmd.rs` test data, `scripts/benchmark/cloud-init.yaml`) — same stable set as earlier reports.
- semgrep p/rust (src): informational only — unsafe-usage (5, libc signal/kill handling), temp-dir (6), current-exe (2), args (2), args-os (1). Pre-existing, no exploitable finding.
- cargo-audit: not installed (osv-scanner + trivy cover Cargo.lock).
