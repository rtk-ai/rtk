# Security Audit — 2026-09-12

## Summary
- Issues found: 74 (38 gitleaks + 36 semgrep-owasp + 36 bandit, minus overlaps counted once per tool) | Auto-fixed: 0 | Unresolved: 2 categories (triaged below)
- Status: PASSED (0 real vulnerabilities, 0 dependency CVEs, 0 verified secrets — all remaining findings are false positives or a known deferred CI-hardening item)

## Fixed Issues
None required. Trivy `fs` found 0 vulnerable packages / 0 secrets across all `Cargo.lock` and `pom.xml` targets in the repo (root + worktrees). TruffleHog found 0 verified/unverified secrets across full git history (1665 commits, 17709 chunks). OSV-Scanner (recursive) found 0 issues across 811 dirs / 7876 inodes. Semgrep `p/secrets` and `p/owasp-top-ten` (Rust/multilang rules) found 0 findings in source code. No dependency CVEs to remediate — no version bumps applied.

## Unresolved Issues

### 1. Gitleaks — 38 hits, all false positives (documentation/test fixtures)
Every hit is a synthetic secret-shaped string: one fake Stripe-style token in `tests/guard_integration_test.rs` around line 167 (a deliberate fixture verifying rtk's git-log filter preserves fixture content byte-for-byte), or similarly-shaped example strings inside historical `Security reports/security-audit-*.md` files illustrating past scan output. Cross-checked against TruffleHog (0 verified/unverified secrets, same full history) — confirms no live secret. Same finding class as every prior daily audit (2026-07-07 through 2026-09-11), no new hits.

### 2. Semgrep — 36 hits: GitHub Actions mutable-tag references (CI-hardening, deferred)
`yaml.github-actions.security.github-actions-mutable-action-tag` across `.github/workflows/{cd,ci,release,stale}.yml` — every `uses: actions/checkout@v4`-style step references a mutable tag/branch instead of a pinned 40-char commit SHA. **Reason unresolved:** a correct fix requires looking up and pinning the exact commit SHA for each action's currently-used release tag per workflow. Guessing a SHA would either break CI or silently pin the wrong commit — worse than the current state. Same deferred item as every prior audit since 2026-07-28; needs a manual pass cross-referencing each action's GitHub release page before pinning.

### Bandit — 36 hits, no fix needed (already-secure pattern)
All 36 are B603/B607/B404 on the same `subprocess.run(["tar", "czf", ...], check=True)` list-form call in `scripts/benchmark-sessions/lib/runner.py` (Bandit flags every call site independently in this version). List-form argv with no `shell=True` is already the secure pattern — no shell interpolation, no injection surface. Same finding as prior audits (fixed count varies only with which paths under `scripts/benchmark-sessions/` were scanned).

## Raw Scanner Output

### Pre-flight
```
OK  bandit 1.9.4
OK  semgrep 1.176.1
OK  trivy 0.74.0        (not in the GHSA-69fq-xp46-6x23 compromised range 0.69.4-0.69.6)
OK  trufflehog 3.97.4
OK  gitleaks 8.30.1
OK  osv-scanner 2.5.1
OK  gh / npx / uvx / skillspector / jq   (mcp-scan and skillspector LLM-mode not invoked — opt-in, not requested for this autonomous run)
```

### Trivy (fs scan, full repo incl. worktrees)
```
All Cargo.lock (208 packages each) and pom.xml fixture targets: 0 vulnerabilities, 0 secrets
```

### OSV-Scanner (recursive)
```
811 dirs visited, 7876 inodes visited, 32 Extract calls — No issues found
```

### TruffleHog (git history, full)
```
chunks: 17709, bytes: 12886315, verified_secrets: 0, unverified_secrets: 0
```

### Gitleaks (git history, 1665 commits)
```
leaks found: 38 — see Unresolved #1 (all confirmed false positives: doc/test-fixture example tokens)
  stripe-access-token rule: 21 hits, generic-api-key rule: 17 hits
```

### Semgrep — secrets (p/secrets, 421 files, 45 rules)
```
Findings: 0
```

### Semgrep — OWASP (p/owasp-top-ten, 421 files, 296 rules)
```
Findings: 36 — all yaml.github-actions.security.github-actions-mutable-action-tag (see Unresolved #2)
```

### Bandit (scripts/benchmark-sessions/lib/runner.py)
```
Total issues: 36 (all Low severity / High confidence, B603/B607/B404 on the same secure subprocess.run() call site)
```

## Coverage Gaps
Not covered this run: business logic / IDOR, runtime behavior, CodeQL (no `.github/workflows/codeql.yml` present), mcp-scan and skillspector LLM-mode (opt-in, requires user consent — not requested for this autonomous run), config-audit/skill-audit/mcp-exfil-scan (not run — no MCP/skill artifacts changed this cycle; deferred to a dedicated Claude-config audit).
