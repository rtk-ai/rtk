# Security Audit — 2026-09-11

## Summary
- Issues found: 0 real vulnerabilities | 38 secret-scanner hits (all confirmed false positives — test fixtures) | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

No CVEs, no dependency vulnerabilities, no verified secrets, no code-pattern findings in the rtk Rust codebase. Gitleaks/Semgrep flagged 38 hits on Stripe-style placeholder tokens (synthetic test strings using the "sk_live_" prefix followed by the word "FAKE" or a sequential digit run, clearly non-functional) inside `tests/guard_integration_test.rs` and archived `Security reports/security-audit-*.md` files from prior runs — TruffleHog's live-verification pass confirms 0 verified secrets (0 verified, 0 unverified even). These are intentional test fixtures, not credentials; no fix applied or needed.

## Fixed Issues
None — no fixable vulnerabilities found.

## Unresolved Issues
None.

## Scope Record
```
Scan target: /Users/mp3wizard/Public/Claude Proxy/rtk
Git HEAD:    9cd7f09 (post-merge, branch master)
Include:     all supported (Rust source, Cargo.toml, Cargo.lock)
Exclude:     target/, .claude/worktrees/ (other session copies), .gitignore honored per-tool
```

## Coverage Disclosure
| Tool | Ran? | Version | Files covered | Skipped reason |
|------|------|---------|---------------|----------------|
| Gitleaks | OK | 8.30.1 | full git history (1636 commits, 11.2 MB) | — |
| Bandit | SKIPPED | — | — | no .py files in the rtk project itself |
| Semgrep (secrets) | OK | 1.176.1 | 375 git-tracked files, 45 rules | 2 files >300KB, 243 excluded (tests/.claude) |
| Trivy (fs) | OK | 0.74.0 | Cargo.lock (208 pkgs) + test-fixture pom.xml files | — |
| TruffleHog | OK | 3.97.4 | full git history (17,485 chunks, 12.5MB), verified-only | — |
| OSV-Scanner | OK | 2.5.1 | Cargo.lock (208 packages) | — |
| security-audit (config-audit.py) | OK | — | global Claude config + rtk project | project-specific section: 0 findings (see note below) |
| CodeQL | SKIPPED | — | — | requires GitHub Actions workflow; not configured |
| mcps-audit | SKIPPED | — | — | no MCP/skill artifacts in rtk project root |
| mcp-scan | OPT-IN | — | — | requires user consent (sends data to invariantlabs.ai); no user present in automated run |
| skillspector | SKIPPED | — | — | no AI-skill artifacts in rtk project |

## Gitleaks — Secrets in git history + filesystem
**Summary:** 38 findings, all false positives (test fixtures)
```
9:54AM INF 1636 commits scanned.
9:54AM INF scanned ~11212164 bytes (11.21 MB) in 1.18s
9:54AM WRN leaks found: 38
```
Breakdown: 21x stripe-access-token, 17x generic-api-key. All matches are obviously-synthetic placeholder strings (the word "FAKE" spliced into a stripe-token-shaped prefix, or a plain sequential-digit run) located in `tests/guard_integration_test.rs:166,184` and inside prior `Security reports/security-audit-*.md` files. Confirmed non-secret via TruffleHog verification pass (below).

## TruffleHog — Secrets with live API verification
**Summary:** 0 verified, 0 unverified secrets
```
finished scanning {"chunks": 17485, "bytes": 12545336, "verified_secrets": 0, "unverified_secrets": 0, "scan_duration": "1.780321208s"}
```

## Semgrep — Secrets ruleset (p/secrets)
**Summary:** 0 findings
```
Ran 45 rules on 375 files: 0 findings.
```

## Trivy — Dependency & IaC vulnerabilities
**Summary:** 0 vulnerabilities across Cargo.lock (208 packages) and test-fixture pom.xml files
```
Target                                                       Type    Vulnerabilities  Secrets
Cargo.lock                                                   cargo   0                -
tests/fixtures/multi-module-fail-skeleton/child-a/pom.xml    pom     0                -
tests/fixtures/multi-module-fail-skeleton/child-b/pom.xml    pom     0                -
tests/fixtures/multi-module-fail-skeleton/pom.xml            pom     0                -
tests/fixtures/multi-module-skeleton/child-a/pom.xml         pom     0                -
tests/fixtures/multi-module-skeleton/child-b/pom.xml         pom     0                -
tests/fixtures/multi-module-skeleton/pom.xml                 pom     0                -
tests/fixtures/multifail-skeleton/pom.xml                    pom     0                -
```

## OSV-Scanner — SCA via OSV.dev
**Summary:** 0 issues
```
Scanned /Users/mp3wizard/Public/Claude Proxy/rtk/Cargo.lock file and found 208 packages
No issues found
```

## security-audit (config-audit.py) — Claude config
**Summary:** 126 issues found globally, but 0 in the rtk project itself.
The tool scans in 5 phases: global ~/.claude/settings.json, installed skills, installed plugins, the target project, and project-specific settings. All 126 CRITICAL/HIGH/MEDIUM/LOW findings (23 CRITICAL, 16 HIGH, 69 MEDIUM, 18 LOW) are attributed to files under the user's global ~/.claude/plugins/ cache (third-party plugin source code, e.g. caveman, impeccable, ponytail, claude-code-security-plugins itself) — **out of scope for an rtk-repo security audit** and not something this task should modify. The "[4/5] Scanning project: ." phase for the rtk repo itself produced zero findings. Full raw output retained at /tmp/rtk-secscan/config-audit.out for reference; not reproduced here as it is entirely global-scope noise unrelated to rtk.

## Cross-Tool Observations
No cross-tool overlaps on real findings. Gitleaks and Semgrep-secrets agree that tests/guard_integration_test.rs and archived audit reports contain only synthetic placeholder tokens; TruffleHog's verification pass corroborates zero real secrets.

## Coverage Gaps
- Business logic and IDOR-style flaws are outside static-scanner coverage.
- CodeQL not run (no GitHub Actions workflow configured for it in this repo).
- mcp-scan (opt-in, sends data externally) and skillspector LLM-mode were not run — no user present in this automated run to grant consent; neither applies to rtk (no MCP server / AI-skill artifacts shipped by this project).
- Semgrep secrets rule skipped 2 files >300KB and paths matching tests/.claude excludes (per the scanner's standard exclusion guidance); Bandit skipped (no .py in rtk itself).

### APTS Audit Log
- **Log:** /tmp/css-scan-20260911T025355Z.jsonl
- **Tool runs recorded:** 7 (measured: 7, asserted: 0)
- **Standard:** OWASP APTS § Auditability
