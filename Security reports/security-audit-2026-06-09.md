# Security Audit — 2026-06-09

## Summary
- Issues found: 0 (production codebase) | Auto-fixed: 0 | Unresolved: 0
- Status: **PASSED**

> All findings from scanners are false positives or limited to stale git worktrees (`.claude/worktrees/`) that are not part of the production build. The main `Cargo.lock` and all Rust source files are clean.

## Fixed Issues
_None — no actionable vulnerabilities found in the production codebase._

## Unresolved Issues
_None._

---

## Raw Scanner Output

### Scope Record
```
Scan target: /Users/mp3wizard/Public/Claude Proxy/rtk
Git HEAD:    de9429b
Include:     all supported
Exclude:     .gitignore honored by each tool
```

### Coverage

| Tool | Ran? | Version | Notes |
|------|------|---------|-------|
| Gitleaks | OK | 8.30.1 | 23 findings — all false positives in audit report docs |
| TruffleHog | OK | 3.94.2 | 0 verified secrets, 0 unverified secrets |
| Semgrep OWASP | OK | latest | 0 findings (182 files) |
| Semgrep Secrets | OK | latest | 0 findings (182 files) |
| Trivy | OK | 0.69.3 | 0 findings in main Cargo.lock; rustls-webpki HIGH in stale worktrees only |
| OSV-Scanner | OK | 2.3.5 | 0 findings in main Cargo.lock |
| Bandit | SKIPPED | — | No Python source files |
| CodeQL | SKIPPED | — | Not triggered (local scan) |
| config-audit | OK | bundled | Findings are FP (scanner scripts + cc-beeper hooks) |
| mcp-exfil-scan | OK | bundled | Findings are FP (scanner scripts + global skills) |

### Gitleaks — Secrets Scan

**Summary:** 23 findings — **all false positives**

All 23 gitleaks findings are in `Security reports/` audit report markdown files from prior
scan runs. They reference example/placeholder strings used to illustrate findings, such as:
- `sk_live_1234567890abcdef` — example Stripe key in audit report prose
- `msk-1234567890abcdef` — benchmark placeholder
- `secret-api-key-12345` — explicit placeholder example

TruffleHog (with live API verification) confirmed **0 verified and 0 unverified real secrets** across 1,103 commits.

### TruffleHog — Verified Secrets Scan

```
chunks: 12500, bytes: 9,163,804
verified_secrets: 0
unverified_secrets: 0
scan_duration: 2.78s
```

**Result: CLEAN**

### Semgrep OWASP Top 10

```
Rules run: 6 (<multilang>), 544 Code rules
Targets scanned: 182 files
Findings: 0
```

**Result: CLEAN**

### Semgrep Secrets

```
Rules run: 36
Targets scanned: 182 files  
Findings: 0
```

**Result: CLEAN**

### Trivy — Filesystem Scan

**Main `Cargo.lock`:** 0 vulnerabilities (203 packages scanned)

**Stale worktrees (`.claude/worktrees/`):** 13 instances of `rustls-webpki 0.103.12`
- Advisory: GHSA-82j2-j2ch-gfr8 / RUSTSEC-2026-0104 (HIGH, CVSS 7.5)
- Issue: DoS via panic on malformed CRL BIT STRING
- Fixed in: 0.103.13
- **Assessment:** These are stale git worktrees created by Claude Code for prior sessions.
  They are NOT part of the production build. The main `Cargo.lock` does not contain this dependency.
  No action required.

### OSV-Scanner — Main Cargo.lock

```
Scanned Cargo.lock: 203 packages
No issues found
```

**Result: CLEAN**

### Config-Audit / MCP-Exfil-Scan

The config-audit and mcp-exfil-scan findings are all false positives:

1. **CRITICAL x5 — Security scanner scripts:** The `security-scanner` and `skill-security-auditor`
   bundled scripts contain patterns (base64, ncat) that they use to *detect* exfiltration — not to
   perform it. These are expected scanner internals.

2. **HIGH — cc-beeper hooks:** `curl` to `localhost:${PORT}` in `settings.json` hooks is the
   intentionally configured cc-beeper notification system for Claude Code session events.
   Destination is `localhost` only (not an external URL).

3. **HIGH — atlas-cloud / skill-security-auditor skills:** These are in the user's global
   `~/.claude/skills/`, not in the rtk project. Out of scope for the rtk production build audit.

**RTK project-specific result: CLEAN**

### APTS Audit Log

```
Log: /tmp/css-scan-20260609T041803Z.jsonl
Tool runs recorded: gitleaks, trivy, trufflehog, semgrep-owasp, semgrep-secrets, osv-scanner, security-audit, mcp-exfil
Standard: OWASP APTS § Auditability
```
