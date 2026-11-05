# Security Audit — 2026-05-21

## Summary
- Issues found: 0 | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

The shipped project (root `Cargo.lock` + `src/`) has no actionable vulnerabilities or live secrets. One HIGH dependency advisory (rustls-webpki) was reported by Trivy but **only inside stale nested worktree repos under `.claude/worktrees/*`** (out of scope, not part of the master branch artifact); the root `Cargo.lock` already pins the fixed version `0.103.13`. All secret-scanner hits are false positives (synthetic test fixtures and example strings in documentation).

## Context
- Upstream sync: merged 8 commits from `origin/develop` (touched `README.md`, `src/hooks/init.rs`).
- Scan target: `/Users/mp3wizard/Public/Claude Proxy/rtk` (master branch).
- Tools run: OSV-Scanner, Trivy (vuln+secret+misconfig), Gitleaks, Semgrep (p/secrets), TruffleHog (verified). Bandit N/A (no Python in scope). cargo-audit not installed — coverage provided by OSV-Scanner + Trivy.

## Fixed Issues
None — no actionable issues required a fix this run.

## Unresolved Issues
None.

## Notable (No Action Required)

### rustls-webpki GHSA-82j2-j2ch-gfr8 (HIGH) — already patched in shipped lockfile
- Advisory: DoS via panic on malformed CRL BIT STRING. Fixed in `0.103.13`.
- Root `Cargo.lock`: **`rustls-webpki 0.103.13`** (already fixed — patched in the 2026-05-20 sync).
- Trivy reported `0.103.12` only in nested git repos under `.claude/worktrees/*` (stale checkouts of other `claude/*` branches). These are gitlink/submodule entries, not tracked content of the master branch, and are not part of the built/shipped binary. No modification of other branches' worktrees was performed.

### Secret-scanner false positives (12 Gitleaks hits, all benign)
| Location | Rule | Nature |
|----------|------|--------|
| `src/cmds/cloud/aws_cmd.rs:1878,1879,1923,2035` | generic-api-key | Synthetic CloudWatch pagination tokens inside `#[cfg(test)]` fixtures (testing that RTK strips them) |
| `scripts/benchmark/cloud-init.yaml:282,613` | generic-api-key | Benchmark fixture data |
| `Security reports/*.md`, `SECURITY.md:151` | stripe-access-token | Example/detector strings inside audit-report and policy documentation |

- Semgrep `p/secrets`: 0 findings (179 targets).
- TruffleHog (verified mode): 0 verified live secrets.

## Raw Scanner Output

### OSV-Scanner (Cargo.lock)
```
Scanned Cargo.lock file and found 203 packages
No issues found
```

### Trivy fs (root project)
```
Root Cargo.lock: rustls-webpki = 0.103.13 (fixed) — 0 vulnerabilities.
HIGH GHSA-82j2-j2ch-gfr8 (rustls-webpki 0.103.12) reported only in:
  .claude/worktrees/*/Cargo.lock  (stale nested repos, out of scope)
```

### Gitleaks
```
988 commits scanned. leaks found: 12 — all false positives (see table above).
```

### Semgrep (p/secrets)
```
Ran 36 rules on 179 files: 0 findings.
```

### TruffleHog (filesystem, verified)
```
0 verified secrets.
```
