# Security Audit — 2026-08-15

## Summary
- Issues found: 0 real (38 gitleaks + 120 trufflehog raw hits, all confirmed false positives) | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

## Fixed Issues
None — no real vulnerabilities found.

## Raw Scanner Output

### Scope Record
- Scan target: `/Users/mp3wizard/Public/Claude Proxy/rtk`
- Git HEAD: `f56dd38` (post-merge, upstream develop synced)
- Include: all supported (Rust source, Cargo.toml, Cargo.lock, tests/, docs)
- Exclude: `.gitignore`-honored per tool

### Coverage Disclosure
| Tool | Ran? | Version | Files covered | Skipped reason |
|------|------|---------|---------------|----------------|
| Gitleaks | OK | 8.30.1 | full git history (1330 commits, 9.09 MB) | — |
| Bandit | SKIPPED | 1.9.4 | — | no `.py` files in target |
| Semgrep (secrets) | OK | latest | 401 git-tracked files | 62 `.semgrepignore`d |
| Trivy (fs) | OK | 0.72.0 | all Cargo.lock/pom.xml fixtures | — |
| TruffleHog | OK | 3.95.9 | full git history (14169 chunks, 10.15 MB) | — |
| OSV-Scanner | OK | 2.4.0 | Cargo.lock (202 packages) + pom.xml fixtures | — |
| CodeQL | SKIPPED | — | — | not configured for this repo/workflow |
| mcps-audit | SKIPPED | — | — | no MCP manifest files in target |
| config-audit (Claude config) | OK | bundled | user-level settings.json, plugin hooks, CLAUDE.md | — |
| skill-audit | SKIPPED | — | — | no SKILL.md/.skill files in target |
| mcp-exfil-scan | SKIPPED | — | — | no MCP files in target |
| skillspector | SKIPPED | — | — | no AI-skill artifacts in target |

### Gitleaks — 38 raw hits, all false positives
All matches are synthetic placeholder strings used in test fixtures and prior audit-report examples:
- `tests/guard_integration_test.rs`: an explicitly `FAKE`-labeled Stripe-shaped test token, used to test the guard-rail filter itself.
- `Security reports/security-audit-*.md` (many dates): illustrative Stripe-token-shaped and generic-API-key-shaped placeholder strings embedded in this task's own historical audit reports.
- `src/cmds/cloud/aws_cmd.rs`: `nextForwardToken`/`nextBackwardToken` fixture values — synthetic AWS CloudWatch Logs API response fixtures for filter unit tests.
- `scripts/benchmark/cloud-init.yaml`: a placeholder API-key-shaped string in benchmark scaffolding.
- `SECURITY.md`: same illustrative placeholder pattern.

No live, real-format credentials found. No action needed.

### TruffleHog — 120 "verified" hits, all false positives
Detector Type: **Lob** (weak/permissive key-format matcher) matched Rust test function names as if they were Lob API keys — e.g. raw result strings that were literally identifiers from `#[test] fn ...` blocks in `src/learn/detector.rs` and `src/cmd/hook/mod.rs` (deduplication test, format-preserving test, transparent-sinks test, multi-file-cat test). These are source-code identifiers, not credentials. TruffleHog's "verified" flag here reflects the Lob detector's format check passing on an alphanumeric-underscore string of matching length, not a live API call succeeding against a real secret. No action needed.

### Semgrep (secrets ruleset)
45 rules run on 401 files — **0 findings**.

### Trivy (filesystem)
All `Cargo.lock` (main repo + 4 active worktrees) and Maven fixture `pom.xml` files scanned — **0 vulnerabilities** in every target.

### OSV-Scanner
202 Cargo dependencies (main `Cargo.lock`) + worktree copies + test-fixture `pom.xml` files scanned against OSV.dev — **no issues found**.

### Config-audit (Claude Code environment)
Findings are all pre-existing, user-level, and outside this repo's remediation scope:
- 🟡 MEDIUM ×4: broad `SessionStart`/`UserPromptSubmit` hook matchers (empty matcher) in installed plugins (`claude-code-security-plugins`, `caveman`) — matches installed-plugin design, not an rtk repo issue.
- 🟡 MEDIUM ×4: `CLAUDE.md`/`claude.md` phrases about trusting snapshot tests and avoiding excessive verification, flagged as "instruction to skip verification" / "trust-all instruction" — these are the repo's own intentional "Avoiding Rabbit Holes" guidance to prevent over-verification loops, not a prompt-injection or safety-bypass risk introduced by this sync.
- 🔵 LOW ×12: informational — hooks configured in various installed plugins.

No CRITICAL/HIGH findings. No action needed.

## Coverage Gaps
- Business logic, IDOR, and runtime behavior are not covered by static scanning.
- CodeQL skipped: no `.github/workflows/codeql.yml` configured for this repo.
- Bandit/skill-audit/mcp-exfil-scan/skillspector skipped: no matching file types present in target (no `.py`, no `SKILL.md`/`.skill`, no MCP manifest files).

### APTS Audit Log
- **Log:** `/tmp/css-scan-20260815T021123Z.jsonl`
- **Tool runs recorded:** 7 (measured: 7, asserted: 0)
- **Standard:** OWASP APTS § Auditability
