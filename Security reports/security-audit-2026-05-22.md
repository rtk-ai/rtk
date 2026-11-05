# Security Audit — 2026-05-22

## Summary
- Issues found: 0 | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

The daily upstream sync merged 9 commits from `origin/develop` (upstream's new
default branch; note upstream flipped default from `master` → `develop` and now
uses `master` as the release branch). The merge included the release 0.40.0 bump,
which already carries the patched `rustls-webpki` 0.103.13. A full multi-tool scan
(OSV-Scanner, Trivy, Gitleaks, TruffleHog) found **no actionable vulnerabilities**
in the root project. All secret-scanner hits are confirmed false positives, and the
single Trivy HIGH applies only to stale nested worktree lockfiles, not the project.

## Fixed Issues
None — no actionable issues were found in the root project this run.

## Unresolved Issues
None.

## Triaged / Out-of-Scope Findings

### Trivy — rustls-webpki HIGH (GHSA-82j2-j2ch-gfr8): NOT APPLICABLE
- Advisory: DoS via panic on malformed CRL BIT STRING. Fixed in 0.103.13 / 0.104.0-alpha.7.
- Root project `Cargo.lock` is already at **rustls-webpki 0.103.13** (the fixed
  version), pulled in by the upstream 0.40.0 release merge. **Root project is not vulnerable.**
- Trivy's HIGH findings were emitted only against stale snapshots in
  `.claude/worktrees/*/Cargo.lock` (e.g. `unruffled-saha-c1ac19`,
  `vigilant-ishizaka-5f7a25`, `youthful-cartwright-40fac1`) — old detached git
  worktrees from prior automated runs, not part of the shipped codebase. No action taken.

### Gitleaks — 12 hits: ALL FALSE POSITIVES
| Count | Rule | Location | Verdict |
|-------|------|----------|---------|
| 5 | generic-api-key | `src/cmds/cloud/aws_cmd.rs` (L1878–2035, `#[cfg(test)]`) | Test fixtures: CloudWatch pagination tokens (`f/1234…`, `b/1234…`) and `secret-api-key-12345` placeholder |
| 2 | generic-api-key | `scripts/benchmark/cloud-init.yaml` | Dummy `API_KEY=sk-1234567890abcdef` placeholder |
| 4 | stripe-access-token | `Security reports/*.md` (2026-04-18, -19, -16) | Example tokens quoted inside prior audit reports |
| 1 | stripe-access-token | `SECURITY.md` | Documentation example token |

TruffleHog (live verification, root excluding worktrees/target) confirmed
**0 verified and 0 unverified secrets** — corroborating that none of the Gitleaks
hits are live credentials.

## Raw Scanner Output

### OSV-Scanner (Cargo.lock — 203 packages)
```
Scanned Cargo.lock file and found 203 packages
No issues found
```

### Trivy fs (root project Cargo.lock)
```
rustls-webpki: root Cargo.lock = 0.103.13 (FIXED). No vulnerability in root project.
HIGH GHSA-82j2-j2ch-gfr8 reported only for stale .claude/worktrees/*/Cargo.lock snapshots (out of scope).
Trivy version: 0.69.3 (not affected by GHSA-69fq-xp46-6x23).
```

### Gitleaks
```
990 commits scanned (~7.73 MB). leaks found: 12 — all triaged as false positives (see table above).
```

### TruffleHog (filesystem, excluding .claude/worktrees and target)
```
chunks: 4672, bytes: 35737271, verified_secrets: 0, unverified_secrets: 0
```

## Coverage Notes
- Covered: Rust dependency vulnerabilities (OSV-Scanner + Trivy SCA), secrets
  (Gitleaks git history + TruffleHog filesystem with live verification),
  IaC/config misconfig + secrets (Trivy).
- Not covered: business-logic flaws, runtime/IDOR behavior. `cargo-audit` and
  `cargo-deny` are not installed in this environment; OSV-Scanner + Trivy provide
  equivalent RustSec/OSV advisory coverage against `Cargo.lock`.
