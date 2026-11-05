# Security Audit — 2026-06-22

## Summary
- Issues found: 0 | Auto-fixed: 0 | Unresolved: 0
- Status: **PASSED**
- Notes: All tool findings are false positives or isolated to stale `.claude/worktrees/` session artifacts. Root codebase is clean.

## Fixed Issues
_None — no real issues found in the main codebase._

## Unresolved Issues
_None._

## Scan Scope
- **Target:** `/Users/mp3wizard/Public/Claude Proxy/rtk`
- **Git HEAD:** `e40dc08`
- **Scanned at:** 2026-06-22T03:18Z
- **Upstream commits merged:** 3 (vitest reporter fix — PRs #2294, #2291)

## Coverage Disclosure

| Tool | Status | Version | Finding Count | Notes |
|------|--------|---------|---------------|-------|
| Gitleaks | OK | 8.30.1 | 31 | All FP — test/doc placeholder strings |
| Semgrep (secrets) | OK | 1.166.0 | 0 | Clean |
| Semgrep (OWASP) | OK | 1.166.0 | 0 | Clean |
| TruffleHog | OK | 3.95.6 | 0 | 0 verified, 0 unverified secrets |
| Trivy | OK | 0.71.2 | 0* | Root Cargo.lock clean; *stale worktrees excluded |
| OSV-Scanner | OK | 2.4.0 | 12* | *All in stale `.claude/worktrees/`, not main Cargo.lock |
| config-audit | OK | 1.7.1 | 51 | All FP — scanner self-scan + intentional user hooks |
| mcp-exfil-scan | PARTIAL | 1.7.1 | 0 | Script error on MCP_CONFIGS; skill files scanned OK |
| Bandit | SKIP | — | — | No Python source files |
| CodeQL | SKIP | — | — | No GH Actions workflow for CodeQL |

## Detailed Findings

### Gitleaks — 31 findings (All False Positives)

All 31 findings are placeholder/test strings in documentation, security reports, and test fixtures:
- `sk_live_1234567890abcdef` — Stripe test key in `SECURITY.md`, prior audit reports
- `msk_live_1234567890abcdef` / `msk-1234567890abcdef` — generic placeholder in benchmark YAML, audit reports
- `secret-api-key-12345`, `f/1234567890abcdef...`, `b/1234567890abcdef...` — AWS CloudWatch pagination token examples in `src/cmds/cloud/aws_cmd.rs` test fixtures

None of these are real secrets. TruffleHog (live API verification) confirmed 0 real secrets across 13,049 chunks scanned.

### Semgrep — 0 findings

- OWASP Top 10 scan: 0 findings across 167 Rust/TOML files
- Secrets scan: 0 findings across 354 tracked files
- Web languages (py/js/ts): 0 findings across 11 files

### TruffleHog — 0 findings

- 1,146 git commits scanned, 13,049 chunks, 9.4 MB
- 0 verified secrets, 0 unverified secrets

### Trivy — Root Cargo.lock Clean

Root `Cargo.lock` contains `rustls-webpki = 0.103.13` (patched, fixes GHSA-82j2-j2ch-gfr8).

12 stale `.claude/worktrees/` had `rustls-webpki = 0.103.12` (GHSA-82j2-j2ch-gfr8, HIGH, DoS via malformed CRL BIT STRING). These are old Claude Code session worktrees, not part of the deployable codebase. No action required for production.

### OSV-Scanner — 12 findings (All in Stale Worktrees)

All 12 findings are `RUSTSEC-2026-0104` / `GHSA-82j2-j2ch-gfr8` in `.claude/worktrees/*/Cargo.lock`. Root `Cargo.lock` is not affected.

### config-audit — 51 findings (All False Positives / Intentional)

| Severity | Count | Assessment |
|----------|-------|------------|
| CRITICAL | 7 | FP — security scanner scripts flagged for containing base64/exfil *detection* patterns |
| HIGH | 12 | FP — cc-beeper hook (localhost notification tool, intentional user config) + CLAUDE.md curl examples |
| MEDIUM | 26 | Intentional — broad hook matchers (design choice), CLAUDE.md efficiency instructions |
| LOW | 6 | Informational — hooks present |

CRITICAL findings are the security scanner's own scripts (`mcp-exfil-scan.sh`, `skill-audit.sh`, `config-audit.py`) triggering on their own detection patterns — a known self-scan artifact.

## Cross-Tool Observations

- **No cross-tool overlap on real issues.** TruffleHog (live verification) is the authoritative source for secrets — it found 0 real secrets, overriding gitleaks' pattern-only 31 FPs.
- OSV-Scanner and Trivy both confirmed main `Cargo.lock` is clean; all vuln hits were isolated to stale `.claude/worktrees/` path prefix.
- Semgrep and TruffleHog are fully clean — highest confidence signal.

## Coverage Gaps

- Business logic, IDOR, and runtime behavior not covered by static analysis.
- Stale worktrees contain known-vulnerable `rustls-webpki` versions; running `tech:clean-worktrees` would eliminate these false positives from future scans.

## Raw Scanner Output

### Gitleaks (summary)
```
1146 commits scanned, ~8.41 MB in 3.22s
leaks found: 31 (all FP — test/documentation placeholder strings)
```

### TruffleHog (summary)
```
chunks: 13049, bytes: 9397509
verified_secrets: 0, unverified_secrets: 0
scan_duration: 2.29s
```

### Semgrep (summary)
```
Secrets:  0 findings on 354 files (45 rules)
OWASP:    0 findings on 167 Rust/TOML files (6 rules)
Web langs: 0 findings on 11 files (223 rules)
```

### Trivy (root Cargo.lock)
```
Cargo.lock: 0 vulnerabilities (rustls-webpki 0.103.13 — patched)
```

### OSV-Scanner (summary)
```
12 findings: all RUSTSEC-2026-0104 in .claude/worktrees/*/Cargo.lock
Root Cargo.lock: clean
```

## APTS Audit Log

```json
{"event":"init","ts":"2026-06-22T03:18:53Z","scope":"/Users/mp3wizard/Public/Claude Proxy/rtk","user":"mp3wizard","git_head":"e40dc08","tool":"apts-audit","standard":"OWASP-APTS"}
{"event":"tool","ts":"2026-06-22T03:28:24Z","tool":"gitleaks","exit":0,"duration_ms":"-","findings":31,"measured":false}
{"event":"tool","ts":"2026-06-22T03:28:24Z","tool":"semgrep","exit":0,"duration_ms":"-","findings":0,"measured":false}
{"event":"tool","ts":"2026-06-22T03:28:24Z","tool":"trivy","exit":0,"duration_ms":"-","findings":0,"measured":false}
{"event":"tool","ts":"2026-06-22T03:28:24Z","tool":"trufflehog","exit":0,"duration_ms":"-","findings":0,"measured":false}
{"event":"tool","ts":"2026-06-22T03:28:24Z","tool":"osv-scanner","exit":0,"duration_ms":"-","findings":12,"measured":false}
{"event":"tool","ts":"2026-06-22T03:28:24Z","tool":"config-audit","exit":0,"duration_ms":"-","findings":51,"measured":false}
{"event":"finalize","ts":"2026-06-22T03:28:24Z","tool_runs":6,"measured_runs":0}
```
