# Security Audit — 2026-09-29

## Scope Record
- **Target:** `/Users/mp3wizard/Public/Claude Proxy/rtk` (branch `master`, merge commit of `origin/master` @ `1d87b8e7`)
- **Git HEAD (pre-scan):** `0557f5cc` → merged to include 3 new upstream commits
- **Include:** all Rust source, `Cargo.toml`, `Cargo.lock`, Python scripts/hooks, `.claude/skills/*/SKILL.md`, Claude config
- **Exclude:** `.git/`, `target/`, files matching `.semgrepignore` (221), files >300KB (1: `src/cmds/git/diff_cmd.rs`)
- **Note:** this session runs from a stale worktree (`amazing-raman-3115e8`); its own `Cargo.lock` was NOT the sync target and is excluded from findings below. All results are against the main checkout's `master`.

## Summary
- Issues found: 0 real | Auto-fixed: 0 | Unresolved: 0
- Status: **PASSED**
- Gitleaks/Bandit/Semgrep/Trivy/TruffleHog/OSV-Scanner all ran clean against `master`. Two skill-audit "CRITICAL" scores were verified as false positives (see below).

## Fixed Issues
None required — no real vulnerabilities found in `master`.

## False Positives (verified, no action taken)
| # | Tool | Location | Why it's a false positive |
|---|------|----------|---------------------------|
| 1 | Gitleaks | `Security reports/security-audit-*.md` (28 hits) | Prior audit reports quoting obviously-fake example/placeholder API-key-shaped strings for illustration — not real secrets |
| 2 | Gitleaks | `tests/guard_integration_test.rs:166,184` | Test fixture using a placeholder token literal with "FAKE" in it |
| 3 | Gitleaks | `SECURITY.md`, `scripts/benchmark/cloud-init.yaml`, one prior dated security report | Same placeholder pattern in docs/fixtures |
| 4 | Gitleaks | `src/cmds/cloud/aws_cmd.rs:1878-2035` | AWS CloudWatch pagination-token sample JSON used in filter test fixtures, not credentials |
| 5 | Bandit | `hooks/hermes/tests/*.py`, `scripts/benchmark-sessions/lib/runner.py` (36 hits, all Low-severity/High-confidence) | `subprocess.run([...])` with list args (no `shell=True`), fixed/known executable names (`cargo`, `tar`) — no untrusted input |
| 6 | skill-audit | `.claude/skills/security-guardian/SKILL.md` → 100/100 CRITICAL | Doc text illustrating a shell-injection *attack example* as the thing the skill teaches you to prevent — not executable code |
| 7 | skill-audit | `.claude/skills/ship/SKILL.md` → 90/100 CRITICAL | Flagged on a documented `cargo install --force` release step and a "No secrets in code" checklist line — no actual secret or dangerous command |
| 8 | OSV-Scanner | 5 RUSTSEC advisories (anyhow, crossbeam-epoch, quick-xml x2, rustls) | All reported against the **stale worktree's** `Cargo.lock` (`.claude/worktrees/amazing-raman-3115e8/Cargo.lock`), not the sync target. Re-scanning `master`'s `Cargo.lock` in isolation (`osv-scanner scan -L`) returned **0 issues** — upstream's merge already carries the fixed versions of all 4 crates |

## Unresolved Issues
None.

## Coverage Disclosure
| Tool | Ran? | Version | Files covered | Skipped reason |
|------|------|---------|---------------|----------------|
| Gitleaks | OK | 8.30.1 | full git history (1907 commits) + working tree | — |
| Bandit | OK | 1.9.4 | 16 `.py` files | — |
| Semgrep (owasp-top-ten) | OK | 1.177.0 | 12 files, 266 rules | 433 non-matching include, 221 semgrepignore |
| Semgrep (python) | OK | — | 2 files, 151 rules | same |
| Semgrep (secrets) | OK | — | 444 files, 45 rules | 1 file >300KB, 221 semgrepignore |
| Trivy (fs) | OK | 0.74.0 | Cargo.lock + all `pom.xml` fixtures | not in compromised version range (0.69.4-6) |
| TruffleHog | OK | 3.97.5 | full git history via `file://` | — |
| OSV-Scanner | OK | 2.6.0 | `master`'s Cargo.lock (isolated re-scan) | initial recursive scan mis-included stale worktree Cargo.lock, corrected |
| config-audit.py | OK | — | `~/.claude/settings.json`, all plugin hooks, `CLAUDE.md` | 0 CRITICAL/HIGH; MEDIUM findings are global plugin-hook noise unrelated to rtk itself |
| skill-audit.sh | OK | — | 12 project `SKILL.md` files | 2 CRITICAL flags verified as false positives (see above) |
| mcp-exfil-scan.sh | SKIPPED | — | — | bundled script failed pre-flight check |
| CodeQL | SKIPPED | — | — | requires `gh run list` against configured workflow; not run this pass |
| mcps-audit | SKIPPED | — | — | no `.mcp*`/MCP manifest files found under target |
| mcp-scan | SKIPPED (opt-in) | — | — | sends data to invariantlabs.ai; no user present to consent (autonomous run) |
| skillspector (LLM mode) | SKIPPED (opt-in) | — | — | privacy-gated; no user present to consent |

## Cross-Tool Observations
No cross-tool overlaps on real findings. The two skill-audit CRITICAL scores were single-tool, pattern-matching false positives (doc text mentioning dangerous shell patterns), not corroborated by Semgrep or Bandit, which scanned the same files and reported clean.

## Coverage Gaps
- Business logic / IDOR: not covered by static scanners.
- CodeQL: skipped (no workflow run triggered this pass).
- mcp-exfil-scan.sh: bundled script errored at pre-flight — Claude config/MCP exfil audit incomplete for this run.
- Runtime behavior: static analysis only; no dynamic/fuzz testing performed.

### APTS Audit Log
- **Log:** `/tmp/css-scan-20260929T145926Z.jsonl`
- **Tool runs recorded:** 11 (measured: 10, asserted: 1)
- **Standard:** OWASP APTS § Auditability
