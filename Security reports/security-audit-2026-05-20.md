# Security Audit — 2026-05-20

## Summary
- Issues found: 1 | Auto-fixed: 1 | Unresolved: 0
- Status: ISSUES FIXED
- Scope: rtk Rust codebase (src/, Cargo.toml, Cargo.lock) after merging 12 upstream commits from `origin/develop`.
- Result: Main project is **clean**. The one HIGH dependency advisory was already resolved in the merged `Cargo.lock` (rustls-webpki 0.103.13). No verified secrets. No unresolved issues.

## Fixed Issues
| # | Component | Advisory | Change |
|---|-----------|----------|--------|
| 1 | rustls-webpki (transitive) | GHSA-82j2-j2ch-gfr8 (HIGH — DoS via panic on malformed CRL BIT STRING) | 0.103.12 → 0.103.13. Already present in the main `Cargo.lock` after the upstream merge; verified via `cargo update -p rustls-webpki --precise 0.103.13` (no-op) and a Trivy re-scan of the main lockfile (clean). |

## Unresolved Issues
None.

## Notes / Non-shipping Findings
- **rustls-webpki 0.103.12** still appears in stale, **untracked** worktree copies under `.claude/worktrees/*/Cargo.lock`. These are local development worktrees (git status: `??`), are not part of the shipped project, and are not committed. No action required.
- **Gitleaks (src/): 5 "generic-api-key" matches** in `src/cmds/cloud/aws_cmd.rs` (lines 1869, 1870, 1914 x2, 2026) are **false positives** — fake CloudWatch pagination tokens (e.g. `f/1234567890abcdef…`) inside `#[cfg(test)]` fixtures. Confirmed by inspection.
- **Gitleaks (full tree): 348 matches** — all from the 7 duplicated worktree copies plus build artifacts; same test-fixture false positives multiplied. Not in the tracked source tree.
- **TruffleHog (src/, verified + unverified): 0 secrets.**

## Raw Scanner Output

### Pre-flight (tool availability)
```
OK  bandit
OK  semgrep
OK  trivy 0.69.3   (safe — not in compromised range 0.69.4–0.69.6)
OK  trufflehog 3.94.2
OK  gitleaks
OK  osv-scanner
MISSING  cargo-audit  (coverage provided by OSV-Scanner + Trivy, both read Cargo.lock)
```

### OSV-Scanner — `osv-scanner scan -L Cargo.lock`
```
Scanned Cargo.lock file and found 203 packages
No issues found
```

### Trivy — `trivy fs --scanners vuln,secret .` (full tree)
```
Total: 1 (HIGH: 1)   — rustls-webpki 0.103.12 → fixed 0.103.13 (GHSA-82j2-j2ch-gfr8)
Occurrences: stale worktree Cargo.lock copies under .claude/worktrees/* (untracked, not shipped).
```

### Trivy — `trivy fs --scanners vuln Cargo.lock` (main lockfile, post-merge / post-update)
```
No vulnerabilities found (rustls-webpki resolved at 0.103.13).
```

### Gitleaks — `gitleaks detect --source ./src --no-git`
```
leaks found: 5  (all generic-api-key in src/cmds/cloud/aws_cmd.rs test fixtures — false positives)
```

### TruffleHog — `trufflehog filesystem ./src --only-verified`
```
chunks: 355, bytes: 2860399, verified_secrets: 0, unverified_secrets: 0, version: 3.94.2
```

## Coverage Gaps
- **cargo-audit** not installed — RustSec advisory DB not queried directly; dependency CVE coverage relied on OSV-Scanner and Trivy (both lockfile-aware). Recommend installing `cargo-audit` for full RustSec parity.
- **Semgrep SAST**: the public Semgrep registry has limited Rust rule coverage; no Rust-specific SAST ruleset was run. Manual review + the project's clippy gate (`cargo clippy --all-targets`) cover idiomatic safety.
- Business-logic flaws, IDOR, and runtime behavior are out of scope for static scanning.
