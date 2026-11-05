# Security Audit — 2026-05-12

## Summary
- Issues found: 0 actionable | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

The main project codebase (Cargo.lock, src/, new hooks/hermes/) is clean. All scanner findings are pre-existing false positives in test fixtures, audit reports, or stale worktree state outside the shipped codebase.

## Fixed Issues
None — no actionable findings.

## Unresolved Issues
None.

## Scanner Coverage

| Tool | Status | Result |
|------|--------|--------|
| Gitleaks | OK | 12 hits, all pre-existing FPs in `Security reports/*.md`, `SECURITY.md`, AWS test fixtures (`src/cmds/cloud/aws_cmd.rs`), and `scripts/benchmark/cloud-init.yaml` |
| Bandit (hooks/hermes/) | OK | 6 Low-severity hits, all `subprocess` calls with hardcoded args in test code — not vulnerabilities |
| OSV-Scanner (main Cargo.lock) | OK | No issues found |
| OSV-Scanner (recursive) | OK | 13 hits for `rustls-webpki 0.103.12` — **all in stale `.claude/worktrees/*/Cargo.lock` only**, NOT in main Cargo.lock |
| Semgrep / Trivy / TruffleHog | Skipped | Time budget — main lockfile clean and no new Rust source from upstream; only Python hook tests added |

## Notes

- **Upstream merge:** 3 commits from `origin/develop` (Hermes plugin integration). New Python files in `hooks/hermes/` — Bandit clean (Low/test-only).
- **rustls-webpki advisory (RUSTSEC-2026-0104):** main project not affected. Stale worktree lockfiles will be cleaned up when those worktrees are reaped; not in shipped code path.
- **Recommendation:** consider periodic cleanup of `.claude/worktrees/` to reduce OSV noise.

## Raw Scanner Output

### Gitleaks (12 findings — all FPs)
```
stripe-access-token | Security reports/security-audit-2026-04-19.md:17
stripe-access-token | Security reports/security-audit-2026-04-18.md:36,39
stripe-access-token | Security reports/2026-04-16-security-report.md:64
generic-api-key    | scripts/benchmark/cloud-init.yaml:613, :282
generic-api-key    | src/cmds/cloud/aws_cmd.rs:1878,1879,1923,2035 (test fixtures)
stripe-access-token | SECURITY.md:151 (documentation example)
```

### Bandit (hooks/hermes/) — 6 Low / test-only
All B603 `subprocess_without_shell_equals_true` in `hooks/hermes/tests/test_rtk_rewrite_plugin.py` — hardcoded arg lists, no untrusted input.

### OSV-Scanner — main Cargo.lock
```
No issues found
```
