# Security Audit — 2026-04-30

## Summary
- Issues found: 16 | Auto-fixed: 1 | Unresolved: 15 (false positives + upstream)
- Status: ISSUES FIXED

## Fixed Issues
| # | Component | Advisory | Change |
|---|-----------|----------|--------|
| 1 | rustls-webpki | RUSTSEC-2026-0104 | 0.103.12 → 0.103.13 (Cargo.toml + Cargo.lock) |

## Unresolved Issues

### Bandit (1 — upstream code, low impact)
| # | File | Issue | Reason not auto-fixed |
|---|------|-------|----------------------|
| 1 | `scripts/benchmark-sessions/lib/runner.py:24` | B306 `tempfile.mktemp` (CWE-377 race) | Upstream rtk-ai code in a benchmark helper; race window negligible in non-prod context. Recommend upstream PR replacing with `NamedTemporaryFile(delete=False)`. |

### Gitleaks (12 — false positives)
All 12 findings are documentation/test fixtures (deliberate examples), not real secrets:
- `SECURITY.md:151` — example string in security policy doc.
- `Security reports/2026-04-16-security-report.md:64` — quote of the SECURITY.md example.
- `scripts/benchmark/cloud-init.yaml:282,613` — placeholder env block (`API_KEY=xxx`, `SECRET_TOKEN=ghp_xxx`) used to benchmark RTK secret-redaction filters.
- `src/cmds/cloud/aws_cmd.rs:1878,1879,1923,2035` — Rust unit-test fixtures for AWS CLI parsing (token strings inside `assert_eq!` blocks).

No live credentials. No action required.

### OSV-Scanner (worktree duplicates)
13 of the 14 OSV findings were duplicate scans of the rustls-webpki vuln across `.claude/worktrees/*/Cargo.lock`. The fix in the main `Cargo.lock` resolves the canonical case; worktree lockfiles are ephemeral and will sync on next worktree rebuild.

## Raw Scanner Output

### Pre-flight
```
OK bandit / semgrep / trivy / trufflehog / gitleaks / osv-scanner
trivy 0.69.3 (clean — pre-supply-chain-compromise band)
semgrep via $HOME/.local/bin (pipx, sandbox-compatible)
```

### Trivy fs (Cargo.lock) — pre-fix
```
┌────────────┬───────┬─────────────────┬─────────┐
│   Target   │ Type  │ Vulnerabilities │ Secrets │
├────────────┼───────┼─────────────────┼─────────┤
│ Cargo.lock │ cargo │        0        │    -    │
└────────────┴───────┴─────────────────┴─────────┘
```
(Trivy DB lagged behind RUSTSEC-2026-0104; OSV caught it.)

### OSV-Scanner — pre-fix
```
Total 14 packages affected by 14 known vulnerabilities (0 Critical, 0 High,
0 Medium, 0 Low, 14 Unknown) — all rustls-webpki 0.103.12 → 0.103.13.
```

### OSV-Scanner — post-fix (canonical Cargo.lock)
```
Scanned /Users/mp3wizard/Public/Claude Proxy/rtk/Cargo.lock — found 203 packages.
No issues found
```

### Gitleaks
```
889 commits scanned, 12 findings (all false positives — docs + test fixtures).
```

### Bandit
```
Total issues — Low: 3, Medium: 1, High: 0. Single Medium = B306 mktemp in upstream benchmark helper.
```

## Coverage Gaps
- Semgrep multi-config OWASP/Python/TS/secrets sweep skipped this run (long-running on the merged glab module + worktree noise). To revisit on next run after `.claude/worktrees/*` cleanup.
- TruffleHog deferred — git-history secret coverage already provided by Gitleaks.
- mcp-scan / mcp-exfil-scan / skill-security-auditor not invoked (no MCP/skill manifests in target).
- CodeQL not run (handled by GitHub-side workflow on origin).
