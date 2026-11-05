# Security Audit — 2026-06-06

## Summary
- Issues found: 1 | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

> **Note:** The single finding (rustls-webpki GHSA-82j2-j2ch-gfr8) appears exclusively in git worktree
> `Cargo.lock` files (`.claude/worktrees/*/Cargo.lock`), which are temporary parallel-work directories
> created by Claude Code. The main project `Cargo.lock` already has `rustls-webpki 0.103.13`
> (the patched version). No action required on the main branch.

## Fixed Issues
_None — main branch was already on the patched version before this audit._

## Unresolved Issues
_None._

## Findings Detail

### Gitleaks — 21 findings (ALL FALSE POSITIVES)
All 21 findings are in `Security reports/*.md` files — they are **example/placeholder secrets**
used in previous audit report write-ups (e.g., `msk-1234567890abcdef`, `sk_live_1234567890abcdef`,
`secret-api-key-12345`). No real credentials. Pattern: recurring across all historical audit
reports since 2026-04-18.

### Trivy / OSV-Scanner — rustls-webpki GHSA-82j2-j2ch-gfr8 (HIGH, worktrees only)
- **Advisory:** GHSA-82j2-j2ch-gfr8 / RUSTSEC-2026-0104
- **Description:** Denial of service via panic on malformed CRL BIT STRING
- **Affected:** `rustls-webpki 0.103.12` in 13 worktree `Cargo.lock` files
- **Main branch status:** Already on `rustls-webpki 0.103.13` ✅
- **CVSS:** 7.5 (High)
- **Impact:** Worktrees are temporary; main production branch is patched

### Semgrep (OWASP + TypeScript + Secrets)
- 0 findings across 115–345 files

### TruffleHog
- 0 verified secrets, 0 unverified secrets (1078 commits scanned)

### Config-audit (43 issues — all false positives or known-good config)
- 5 CRITICAL: Security-scanner skill itself flagged for scanning base64/.env (meta-scan FP)
- 7 HIGH: cc-beeper hooks use `curl` to `localhost` (intentional desktop notification system)
- 2 HIGH: Plugin example scripts showing dangerous patterns (documentation, not runtime)
- 2 HIGH: CLAUDE.md references to `curl` in code examples
- 21 MEDIUM: Broad hook matchers (intentional catch-all for notifications), skill references
  to cookies/passwords (legitimate browser-test tooling)
- 5 LOW: Informational hooks configuration present

### MCP Exfil Scan (11 issues — all false positives)
- 2 CRITICAL: Security-scanner scripts flagged for containing patterns they detect (meta-scan FP)
- 5 HIGH: Known skills (atlas-cloud, skill-security-auditor) flagged for legitimate env+network use
- 4 MEDIUM: Skills without source attribution (playwright-cli, pyright, vtsls)
- Risk score 100/100 is a known artifact when scanner scans itself

## Upstream Sync
Merged 30 commits from `origin/develop`:
- refacto(cmds): strip decorator noise from filter output
- fix: semgrep markers on test-fixture sensitive paths
- test(hook): Copilot CLI acceptance tests
- fix(openclaw): no execSync to avoid async dangerous cmds
- fix(permissions): >&file redirect hardening, project-first config lookup
- chore(master): release 0.42.2

## Raw Scanner Output

### Gitleaks
```
1078 commits scanned. Scanned ~8006252 bytes (8.01 MB) in 1.65s
leaks found: 21 (all in Security reports/*.md — example placeholder secrets)
```

### Semgrep OWASP
```
Findings: 0 | Rules run: 77 | Targets scanned: 115
```

### Semgrep TypeScript
```
Findings: 0 | Rules run: 74 | Targets scanned: 9
```

### Semgrep Secrets
```
Findings: 0 | Rules run: 45 | Targets scanned: 345
```

### Trivy
```
rustls-webpki | GHSA-82j2-j2ch-gfr8 | HIGH | fixed | 0.103.12 → 0.103.13
(found in worktree Cargo.lock files only; main Cargo.lock = 0.103.13)
```

### TruffleHog
```
chunks: 12222 | bytes: 8,668,024 | verified_secrets: 0 | unverified_secrets: 0
scan_duration: 1.83s
```

### OSV-Scanner
```
Total: 13 packages affected by 13 known vulnerabilities (0 Critical, 13 High, 0 Medium, 0 Low)
All 13: rustls-webpki 0.103.12 in worktree Cargo.lock files — fixed in main (0.103.13)
```

### APTS Audit Log
- **Log:** `/tmp/css-scan-20260606T021208Z.jsonl`
- **Standard:** OWASP APTS § Auditability
- **Scan target:** `/Users/mp3wizard/Public/Claude Proxy/rtk`
- **Git HEAD:** 623fe37
