# Security Audit — 2026-05-10

## Summary
- Issues found: 0 (in-scope) | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED
- Note: 13 advisory matches reported by OSV-Scanner are all located in `.claude/worktrees/*/Cargo.lock` (transient scratch worktrees). The main `Cargo.lock` already pins `rustls-webpki = 0.103.13` (RUSTSEC-2026-0104 fix). No action required for the shipping codebase.

## Fixed Issues
None — main codebase clean.

## Advisory Findings (Out-of-Scope, Informational)
| # | Component | Advisory | Location | Status |
|---|-----------|----------|----------|--------|
| 1 | rustls-webpki 0.103.12 → 0.103.13 | RUSTSEC-2026-0104 / GHSA-82j2-j2ch-gfr8 (CVSS 7.5) | 13 stale worktrees under `.claude/worktrees/` | N/A — main `Cargo.lock` already at 0.103.13 |

These worktrees are user-local scratch and not part of the build/release artifact.

## Scanners Run
| Tool | Result |
|------|--------|
| Gitleaks | 0 leaks |
| Trivy (fs vuln+secret+misconfig, target+worktrees skipped) | 0 findings |
| OSV-Scanner (Cargo.lock root) | No issues |
| OSV-Scanner (recursive incl. worktrees) | 13 advisory matches — all in stale worktrees (see above) |
| TruffleHog (filesystem, verified) | 0 secrets |
| Semgrep (p/secrets) | 0 findings |

## Upstream Sync Context
- Merged 28 upstream commits from `origin/develop` into `master` (commit `986831e` pre-merge HEAD).
- Notable additions: gradlew filter (jvm), dotnet output formatting, init `--dry-run`, hooks `transparent_prefixes` config.

## Raw Scanner Output

### Gitleaks
```
(no leaks found)
```

### Trivy
```
(no findings)
```

### OSV-Scanner (root Cargo.lock)
```
Scanned /Users/mp3wizard/Public/Claude Proxy/rtk/Cargo.lock file and found 203 packages
No issues found
```

### OSV-Scanner (recursive — worktree noise only)
13 × rustls-webpki 0.103.12 (RUSTSEC-2026-0104) under `.claude/worktrees/*/Cargo.lock`. Main lockfile = 0.103.13 (already fixed).

### TruffleHog
```
chunks: 3193  verified_secrets: 0  unverified_secrets: 0
```

### Semgrep p/secrets
```
(no findings)
```
