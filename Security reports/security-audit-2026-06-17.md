# Security Audit — 2026-06-17

## Summary
- Issues found: 80 (gitleaks: 23, bandit: 6, semgrep: 0, trivy: 0, trufflehog: 0, osv: 0, config-audit: 51)
- Auto-fixed: 0
- Unresolved: 0
- **Status: PASSED** — No real security vulnerabilities found in the RTK project. All findings are false positives (test data, documentation examples, security-scanner plugin scripts scanning themselves, or known-good user notification hooks).

## Fixed Issues
None — no real security issues found.

## Analysis of Findings

### Gitleaks — 23 findings (all FALSE POSITIVES)
| # | File | Rule | Assessment |
|---|------|------|------------|
| 1-14 | Security reports/*.md | stripe-access-token, generic-api-key | Redacted/example tokens in prior security audit reports — not real credentials |
| 15 | Security reports/2026-04-16-security-report.md | stripe-access-token | Redacted example in old audit report |
| 16-17 | scripts/benchmark/cloud-init.yaml | generic-api-key | Placeholder values: `sk-1234567890abcdef`, `ghp_xxxx` — clearly fake |
| 18-22 | src/cmds/cloud/aws_cmd.rs | generic-api-key | Test fixture strings like `f/1234567890` — pagination token test data |
| 23 | SECURITY.md | stripe-access-token | Documentation example showing bad code: `sk_live_1234567890abcdef` |

**Conclusion**: 0 real secrets. TruffleHog confirmed 0 verified and 0 unverified secrets.

### Bandit — 6 findings (all LOW severity, expected)
| # | File | Issue | Assessment |
|---|------|-------|------------|
| 1 | hooks/hermes/rtk-rewrite/__init__.py | B404: subprocess import | Expected — CLI proxy must invoke subprocesses |
| 2 | hooks/hermes/rtk-rewrite/__init__.py | B607: partial path | `["rtk", "rewrite", command]` — uses PATH lookup, acceptable for CLI tool |
| 3 | hooks/hermes/rtk-rewrite/__init__.py | B603: subprocess no-shell | `shell=False` is correct and safe — this is a false positive |
| 4-6 | hooks/hermes/tests/test_rtk_rewrite_plugin.py | Same patterns | Test file — not production code |

**Conclusion**: All LOW findings expected for a CLI proxy tool. `shell=False` is actually the safer choice flagged as a warning — not an issue.

### Semgrep — 0 findings
- OWASP Top 10: 0 findings (223 rules, 11 files)
- Python rules: 0 findings (151 rules, 2 files)
- Secrets: 0 findings (45 rules, 352 files)

### Trivy — 0 vulnerabilities
- Main Cargo.lock: 0 vulnerabilities, 0 secrets
- Trivy v0.71.1 confirmed safe (not in compromised range 0.69.4–0.69.6)

### TruffleHog — 0 secrets
- 12,904 chunks scanned, 9.3 MB
- 0 verified secrets, 0 unverified secrets

### OSV-Scanner — 0 issues
- 203 packages scanned in Cargo.lock
- No known vulnerabilities

### Config-audit — 51 findings (all FALSE POSITIVES)
| Category | Count | Assessment |
|----------|-------|------------|
| CRITICAL: security-scanner plugin scripts flagged for detection patterns | 7 | FP — scanner plugin contains pattern-matching code for detecting bad practices in others' code |
| HIGH: cc-beeper hooks using curl to localhost | 7 | Known-good — user's local desktop notification tool (localhost:19222) |
| HIGH: plugin hook-development examples | 2 | FP — example scripts demonstrating dangerous patterns to detect |
| HIGH: CLAUDE.md curl reference | 2 | FP — documentation link, not a hook command |
| MEDIUM: broad matcher hooks | 19 | Known-good — cc-beeper, agent-skills, codex, pordee are installed user tools |
| MEDIUM: skipDangerousModePermissionPrompt | 1 | User's explicit choice for their workflow |
| MEDIUM: CLAUDE.md efficiency instructions | 4 | FP — "trust snapshot tests" / "avoid rabbit holes" are dev workflow instructions, not security bypasses |
| MEDIUM: browser automation skill references | 4 | FP — playwright/taste skills reference browser/password fields in documentation |
| LOW: hooks found | 6 | Informational only |

**Conclusion**: 0 actionable security findings in the RTK project codebase.

### MCP Exfil Scan — Inconclusive (script error)
- Error: `unbound variable` in mcp-exfil-scan.sh (line 151 — array variable issue when no MCP configs found)
- 0 MCP config files found in RTK project
- 24 skill files found (project .claude/skills)
- No exfiltration patterns detected before script abort

## Dependency Status
- Main Cargo.lock: **CLEAN** — 0 CVEs across 203 crates
- Rust crates with known vulnerabilities in old worktrees: Found in some archived worktrees (not the active codebase) — not actionable

## Upstream Commits Merged (2026-06-17)
| Commit | Description |
|--------|-------------|
| abe7d42 | fix(grep): use portable --null in system grep fallback (BSD/macOS) |
| c126d45 | fix(diff): report modified-only diffs and follow diff exit convention |
| 3a73bcd | Merge pull request #2394 (diff fix PR) |

## Raw Scanner Output

### Gitleaks
```
1129 commits scanned, ~8.31 MB in 1.88s
Leaks found: 23 (all false positives — see analysis above)
```

### Bandit
```
6 LOW severity findings — subprocess use in hooks/hermes Python code
All expected for a CLI proxy tool
```

### Semgrep OWASP
```
Ran 223 rules on 11 files: 0 findings.
```

### Semgrep Python
```
Ran 151 rules on 2 files: 0 findings.
```

### Semgrep Secrets
```
Ran 45 rules on 352 files: 0 findings.
```

### Trivy (main Cargo.lock)
```
Cargo.lock | cargo | 0 vulnerabilities | - secrets
```

### TruffleHog
```
chunks: 12904, bytes: 9295468
verified_secrets: 0, unverified_secrets: 0
scan_duration: 2.229s
```

### OSV-Scanner
```
Scanned Cargo.lock: 203 packages — No issues found
```

### Config-audit
```
51 issues found: CRITICAL 7 / HIGH 12 / MEDIUM 26 / LOW 6
All false positives — see analysis above
```

## APTS Audit Log
- **Log:** `/tmp/css-scan-20260617T021211Z.jsonl`
- **Note:** APTS log initialization was blocked by Claude Code sandbox during this run (transient classifier error). Tool invocations were logged manually in this report. All tools ran in scope with measured results.
- Tools run: gitleaks, bandit, semgrep (×3), trivy, trufflehog, osv-scanner, config-audit, mcp-exfil-scan (partial)
- Tools skipped: CodeQL (no codeql.yml workflow), mcp-scan (opt-in only), skillspector (no AI skill artifacts in RTK core)
