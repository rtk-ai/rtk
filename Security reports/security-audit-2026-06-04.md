# Security Audit — 2026-06-04

## Summary
- Issues found: 0 | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

All scanner findings in the main codebase are false positives (test fixtures, benchmark data, documentation examples).
The upstream merge brought in 5 commits including multibyte-char panic fix and grep command deduplication fix.

## Fixed Issues
None. The main `Cargo.lock` already carries `rustls-webpki` 0.103.13 (patched, GHSA-82j2-j2ch-gfr8). No action required.

## Detailed Findings (all false positives / informational)

### Gitleaks — 21 "leaks" detected (all false positives)
| # | File | Rule | Assessment |
|---|------|------|------------|
| 1-12 | `Security reports/security-audit-*.md` | generic-api-key, stripe-access-token | Previous scanner output embedded in reports |
| 13-14 | `scripts/benchmark/cloud-init.yaml:282,613` | generic-api-key | Intentional test fixture data (`sk-1234567890abcdef`, `ghp_xxxx`) for benchmark environment |
| 15-19 | `src/cmds/cloud/aws_cmd.rs:1878-2035` | generic-api-key | Unit test fixture data for AWS CloudWatch log filtering |
| 20 | `SECURITY.md:151` | stripe-access-token | Documentation example of bad practice (`sk_live_1234567890abcdef`) |
| 21 | `Security reports/2026-04-16-security-report.md` | stripe-access-token | Previous scanner output |

**Conclusion**: Zero real credentials. All are test data, benchmark fixtures, or documentation examples.

### Semgrep — 0 findings
- OWASP Top 10: 0 findings across 12 files (Python, TypeScript, Ruby, Go)
- Secrets: 0 findings across 344 files

### TruffleHog — 0 verified/unverified secrets
- Scanned 1060 commits, 12,055 chunks, 8.6 MB
- 0 verified secrets, 0 unverified secrets

### Trivy / OSV-Scanner — rustls-webpki (worktrees only)
- **RUSTSEC-2026-0104 / GHSA-82j2-j2ch-gfr8** (HIGH, CVSS 7.5)
- Denial of service via panic on malformed CRL BIT STRING
- Found in: `.claude/worktrees/*/Cargo.lock` (old branch worktrees — NOT main codebase)
- **Main `Cargo.lock` status**: Already on `rustls-webpki` 0.103.13 (patched) ✓
- Action: None required for main codebase. Worktrees will be cleaned up as branches are removed.

### Config Audit — Informational only
- Low: Hook configurations present (expected — RTK uses Claude Code hooks)
- Medium: CLAUDE.md "skip verification" pattern (expected — intentional workflow guidance to avoid rabbit holes)

### MCP Exfil Scan — All false positives
- CRITICAL: Security audit skills flagged for mentioning "exfiltration" in their descriptions (they ARE security tools)
- HIGH: skill-security-auditor flagged for having Bash+WebFetch (required for security auditing)
- These findings are outside RTK project scope and relate to the user's global Claude skills configuration

## Upstream Commits Merged (5)
| SHA | Description |
|-----|-------------|
| 4f4a6a0 | Merge PR #1266: fix panic on multibyte chars in commit output |
| 57e9350 | Merge PR #2239: restore double-dash fix |
| 89ae19b | fix(grep): command token duplication in output with same value filename or positional arg |
| ab4fa7b | test(git): add regression tests for commit output parsing with multibyte branch names |
| c5ec92f | fix(git): fix panic on multibyte chars in commit output |

## Raw Scanner Output

### Gitleaks
```
1060 commits scanned.
scanned ~7929297 bytes (7.93 MB) in 1.65s
leaks found: 21 (all false positives — see table above)
```

### Semgrep OWASP
```
✅ Scan completed successfully.
 • Findings: 0 (0 blocking)
 • Rules run: 266
 • Targets scanned: 12
```

### Semgrep Secrets
```
✅ Scan completed successfully.
 • Findings: 0 (0 blocking)
 • Rules run: 45
 • Targets scanned: 344
```

### TruffleHog
```
chunks: 12055, bytes: 8585559
verified_secrets: 0, unverified_secrets: 0
scan_duration: 1.887165292s
```

### Trivy (main Cargo.lock)
Main Cargo.lock: rustls-webpki 0.103.13 — no vulnerabilities.
Worktrees: rustls-webpki 0.103.12 (GHSA-82j2-j2ch-gfr8, HIGH) — worktrees only.

### OSV-Scanner
RUSTSEC-2026-0104 / GHSA-82j2-j2ch-gfr8 found in 12 worktree Cargo.lock files only.
Main Cargo.lock: clean.

### APTS Audit Log
- Log: `/tmp/css-scan-20260604T021516Z.jsonl`
- Tool runs recorded: 5
- Standard: OWASP APTS § Auditability
