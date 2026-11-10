# Security Audit — 2026-05-14

## Summary
- Issues found: 0 | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

Daily upstream sync merged 7 commits from `origin/develop` (cleanly, no conflicts).
Scanned all Rust sources, `Cargo.toml`, and `Cargo.lock` with Trivy, OSV-Scanner,
Gitleaks, and TruffleHog. No actionable vulnerabilities in the project.

## Fixed Issues
None — no actionable issues found.

## Unresolved Issues
None.

## Notes
- **rustls-webpki / GHSA-82j2-j2ch-gfr8 (HIGH)** — Trivy flagged this DoS advisory,
  but only in stale `.claude/worktrees/*/Cargo.lock` snapshots. The project's root
  `Cargo.lock` already pins `rustls-webpki 0.103.13` (the patched version). OSV-Scanner
  reported no issues on the root lockfile (203 packages). No action required; the
  worktree copies are ephemeral and not part of the build.
- **Gitleaks: 12 findings (git history)** — all false positives: redacted/example
  Stripe tokens in prior `Security reports/*.md` and `SECURITY.md`, and example AWS
  keys used as test fixtures in `src/cmds/cloud/aws_cmd.rs` and
  `scripts/benchmark/cloud-init.yaml`. TruffleHog confirmed **0 verified and 0
  unverified** secrets across 11,259 chunks.

## Raw Scanner Output

### Trivy (fs, root Cargo.lock)
```
Total: 0 (root Cargo.lock — rustls-webpki 0.103.13, no vulnerabilities)
HIGH findings limited to .claude/worktrees/*/Cargo.lock (stale snapshots, out of scope):
  rustls-webpki 0.103.12  GHSA-82j2-j2ch-gfr8  HIGH  fixed in 0.103.13
```

### OSV-Scanner (Cargo.lock)
```
Scanned Cargo.lock file and found 203 packages
No issues found
```

### Gitleaks
```
963 commits scanned. ~7.17 MB. leaks found: 12 (all git history, all false positives)
- stripe-access-token  Security reports/security-audit-2026-04-19.md
- stripe-access-token  Security reports/security-audit-2026-04-18.md  (x2)
- stripe-access-token  Security reports/2026-04-16-security-report.md
- stripe-access-token  SECURITY.md
- generic-api-key      scripts/benchmark/cloud-init.yaml  (x2)
- generic-api-key      src/cmds/cloud/aws_cmd.rs  (x5)
```

### TruffleHog (git, verified + unverified)
```
chunks: 11259, bytes: 7788402, verified_secrets: 0, unverified_secrets: 0
```
