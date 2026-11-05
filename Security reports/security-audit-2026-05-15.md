# Security Audit — 2026-05-15

## Summary
- Issues found: 0 | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

Upstream sync merged 4 commits from `origin/develop`. Full scan (gitleaks, trivy, osv-scanner, trufflehog) found no actionable vulnerabilities in the project. The upstream merge already carries `rustls-webpki 0.103.13`, which resolves GHSA-82j2-j2ch-gfr8.

## Fixed Issues
None — no actionable issues found.

## Unresolved Issues
None.

## Notes (non-actionable)
- **Trivy** reported `rustls-webpki 0.103.12` (GHSA-82j2-j2ch-gfr8, HIGH) — but only inside stale `.claude/worktrees/*/Cargo.lock` build artifacts. The project root `Cargo.lock` is already at the patched `0.103.13`, and `osv-scanner` on the root lockfile reports no issues.
- **Gitleaks** flagged 12 entries, all false positives: example/fixture pagination tokens in `src/cmds/cloud/aws_cmd.rs` test code, a benchmark fixture in `scripts/benchmark/cloud-init.yaml`, and illustrative secret strings inside `SECURITY.md` and prior `Security reports/*.md` documents.

## Raw Scanner Output

### Pre-flight
```
bandit 1.9.4 | semgrep | trivy 0.69.3 | trufflehog 3.94.2 | gitleaks 8.30.1 | osv-scanner 2.3.5
```

### Trivy (fs, vuln+secret+misconfig)
```
Root project: no actionable findings.
rustls-webpki GHSA-82j2-j2ch-gfr8 (HIGH) — present ONLY in:
  .claude/worktrees/vigilant-ishizaka-5f7a25/Cargo.lock
  .claude/worktrees/youthful-cartwright-40fac1/Cargo.lock
  (+ other stale worktree lockfiles)
  Installed 0.103.12 → Fixed 0.103.13. Not present in root Cargo.lock.
```

### OSV-Scanner (root Cargo.lock)
```
Scanned Cargo.lock — 203 packages. No issues found.
```

### Gitleaks
```
966 commits scanned, 12 leaks found — all false positives:
  generic-api-key  scripts/benchmark/cloud-init.yaml:282,613   (benchmark fixture)
  generic-api-key  src/cmds/cloud/aws_cmd.rs:1878,1879,1923,2035 (test fixture pagination tokens)
  stripe-access-token  SECURITY.md:151                          (doc example)
  stripe-access-token  Security reports/2026-04-16-security-report.md:64
  stripe-access-token  Security reports/security-audit-2026-04-18.md:36,39
  stripe-access-token  Security reports/security-audit-2026-04-19.md:17
```

### Root Cargo.lock confirmation
```
name = "rustls-webpki"
version = "0.103.13"
```
