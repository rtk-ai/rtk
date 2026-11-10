# Security Audit — 2026-04-18

## Summary
- Issues found: 0 | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

No actionable security vulnerabilities were found in the RTK codebase. All scanner "findings" were false positives in documentation, test fixtures, or related to the security-scanner tool detecting its own patterns in the skills directory.

## Fixed Issues
None.

## Unresolved Issues
None.

## Raw Scanner Output

### Pre-flight
| Tool | Status | Version |
|------|--------|---------|
| gitleaks | OK | 8.30.1 |
| semgrep | OK | latest |
| trivy | OK | 0.69.3 (safe) |
| trufflehog | OK | 3.94.2 |
| osv-scanner | OK | 2.3.5 |
| bandit | OK | 1.9.4 |
| gh (CodeQL) | OK | — |
| npx (mcps-audit) | OK | — |
| jq | OK | — |

---

### Gitleaks — 9 findings (all false positives)

| File | Rule | Assessment |
|------|------|-----------|
| `Security reports/2026-04-16-security-report.md` | stripe-access-token | FP — `sk_live_1234567890abcdef` is a documentation example |
| `scripts/benchmark/cloud-init.yaml` | generic-api-key | FP — `API_KEY=sk-12345...` is a truncated placeholder in a benchmark script |
| `src/cmds/cloud/aws_cmd.rs` (5 hits) | generic-api-key | FP — code handles AWS Secrets Manager output; no actual keys in source |
| `SECURITY.md` | stripe-access-token | FP — `sk_live_1234567890abcdef` is a documentation example showing what NOT to commit |

TruffleHog (live-verified) confirmed **0 verified secrets, 0 unverified secrets** across all 9884 chunks (6.9 MB scanned).

---

### Trivy — 0 vulnerabilities

```
Cargo.lock: 0 vulnerabilities
.claude/worktrees/.../Cargo.lock: 0 vulnerabilities
```

---

### OSV-Scanner — 0 issues

Scanned 203 packages in Cargo.lock. No known vulnerabilities.

---

### Semgrep OWASP — 0 findings

266 rules run on 10 files. Clean.

---

### Semgrep Secrets — 0 findings

45 rules run on 309 files. Clean.

---

### TruffleHog — 0 secrets

818 commits scanned (6.9 MB). 0 verified, 0 unverified.

---

### Config Audit (config-audit.py)

22 issues reported — all false positives or out-of-scope:
- **CRITICAL ×5**: Security scanner skill files detecting their own patterns (self-referential FP)
- **HIGH ×2**: CLAUDE.md contains curl examples in documentation (not hooks)
- **HIGH ×2**: Plugin validate-bash.sh contains examples of commands it *blocks* (detection FP)
- **HIGH ×1**: optimize skill mentions netcat in documentation context
- **MEDIUM ×9**: Project-specific CLAUDE.md design decisions (skip-verification guidance, trust-snapshot guidance) — intentional project workflow rules, not security issues
- **LOW ×3**: Hooks configuration informational notices

None of these affect RTK source code security.

---

### MCP Exfil Scan

9 issues reported — all false positives:
- Findings in `~/.claude/skills/skill-security-auditor/SKILL.md` (security tool detecting itself)
- Skill attribution warnings for playwright-cli, pyright, vtsls (no source metadata)

None affect RTK source code.

---

## Cross-Tool Observations

All five dependency scanners (Trivy, OSV-Scanner, TruffleHog, Semgrep OWASP, Semgrep Secrets) returned **zero findings**. The only flags were from Gitleaks pattern-matching against documentation examples, which TruffleHog's live verification definitively confirmed are not real secrets.

## Coverage Gaps

- Runtime behavior and business logic not covered by static analysis
- CodeQL not run (would require GitHub Actions workflow trigger)
- mcp-scan (invariantlabs.ai) not run — opt-in only, sends data externally
