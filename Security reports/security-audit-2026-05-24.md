# Security Audit — 2026-05-24

## Summary
- Issues found: 0 (main codebase) | Auto-fixed: 0 | Unresolved: 0
- Worktree noise: 13 informational findings in stale development worktrees (not production code)
- Status: PASSED

> **Note on worktree findings:** Trivy and OSV-Scanner scanned all `.claude/worktrees/*/Cargo.lock` files and found `rustls-webpki 0.103.12` (GHSA-82j2-j2ch-gfr8, HIGH) in 12 old worktrees. These are ephemeral development sandboxes — not deployed or installed. The **main `Cargo.lock` is clean** (0 vulnerabilities). No fix applied to worktrees.

## Fixed Issues
_None — no actionable vulnerabilities in the active codebase._

## Unresolved Issues
_None._

## Informational: Worktree-only Findings (not production code)

| Advisory | Severity | Package | Affected Version | Fixed In | Location |
|----------|----------|---------|-----------------|----------|----------|
| GHSA-82j2-j2ch-gfr8 / RUSTSEC-2026-0104 | HIGH | rustls-webpki | 0.103.12 | 0.103.13 | 12 stale worktrees only |

All 12 occurrences are in `.claude/worktrees/*/Cargo.lock` — ephemeral git worktrees used during development sessions. The main `Cargo.lock` resolved this crate to a safe version.

## Raw Scanner Output

### Scope Record
```
Scan target: /Users/mp3wizard/Public/Claude Proxy/rtk
Git HEAD:    870b3d3
Include:     all supported
Exclude:     .gitignore honored by each tool
```

### Tool Coverage
| Tool | Status | Result |
|------|--------|--------|
| Gitleaks 8.30.1 | RAN | 13 false positives (test fixtures/placeholder keys only) |
| Semgrep (OWASP + Secrets) | RAN | 0 findings |
| Trivy 0.69.3 | RAN | 0 findings in main Cargo.lock; 12 worktree-only HIGH (rustls-webpki) |
| TruffleHog 3.94.2 | RAN | 0 verified or unverified secrets |
| OSV-Scanner 2.3.5 | RAN | 13 findings — all in stale worktrees; main Cargo.lock clean |
| Bandit 1.9.4 | SKIPPED | No Python source files in Rust project |
| CodeQL | SKIPPED | No GitHub Actions workflow detected |
| security-audit (config-audit.py) | RAN | 43 config-scope flags — all known-benign (cc-beeper notification hooks, security tools self-flagging) |
| mcp-exfil-scan | RAN | 11 flags — all security scanner tools self-referential (expected) |
| mcp-scan | OPT-IN | Not run (requires user consent; sends data to invariantlabs.ai) |

### Gitleaks Detail (all false positives)
All 13 findings are test fixtures or documentation placeholder values:
- `Security reports/*.md`: example keys like `sk-1234567890abcdef` in audit report examples
- `scripts/benchmark/cloud-init.yaml`: `API_KEY=sk-1234567890abcdef` benchmark placeholder
- `src/cmds/cloud/aws_cmd.rs`: fake AWS tokens in test fixtures (L1878-2035)
- `SECURITY.md`: example `sk_live_1234567890abcdef` in security documentation

### TruffleHog
0 verified secrets, 0 unverified secrets across 1010 commits scanned.

### Trivy (main Cargo.lock)
Main Cargo.lock: 0 vulnerabilities detected. All clean.

### Semgrep
OWASP Top 10: 0 findings across 12 files.
Secrets ruleset: 0 findings across 337 files.

### OSV-Scanner (main lockfile only)
Main `Cargo.lock` at project root: 0 vulnerabilities.

### APTS Audit Log
- **Log:** `/tmp/css-scan-20260524T023046Z.jsonl`
- **Tool runs recorded:** 9
- **Standard:** OWASP APTS § Auditability
