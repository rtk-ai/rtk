# Security Audit — 2026-09-23

## Summary
- Issues found: 1 | Auto-fixed: 1 | Unresolved: 0
- Status: ISSUES FIXED

Scope: full rtk codebase (all Rust sources, `Cargo.toml`, `Cargo.lock`) after merging 92 upstream commits (`origin/develop` -> `master`). 126 gitleaks/config-audit/skill-audit hits were investigated and confirmed false positives (see Cross-Tool Observations) - not counted as issues.

## Fixed Issues
| # | Component | Advisory | Change |
|---|-----------|----------|--------|
| 1 | rustls | RUSTSEC-2026-0285 (CVSS 5.3, Medium) | `cargo update -p rustls`: `0.23.37` -> `0.23.45` (also pulled `rustls-webpki` `0.103.13` -> `0.103.15`, transitive) |

**Merge-induced build break (not a security finding, fixed to keep the tree green):** the upstream merge conflicted in [src/cmds/dotnet/dotnet_cmd.rs](../src/cmds/dotnet/dotnet_cmd.rs) between this repo's `quick-xml 0.41` API (`BytesText::decode()`) and upstream's `.unescape()` call (removed in `0.41`, see prior audit `security-audit-2026-07-07.md`). Conflict was first resolved by accepting upstream's side per task policy, which then failed `cargo clippy` (`E0599: no method named unescape`); corrected the one call site back to `.decode()` to match this project's pinned `quick-xml` version. `cargo fmt --all && cargo clippy --all-targets && cargo test --all` all pass (3920 passed, 8 ignored).

## Unresolved Issues
None.

## Raw Scanner Output

### Gitleaks (git history + filesystem)
38 findings, all confirmed false positives - placeholder/example tokens in prior audit report docs (`Security reports/security-audit-*.md`, `Security reports/2026-04-16-security-report.md`), `SECURITY.md`'s own vulnerable-code example (a fake Stripe-shaped live-key literal used to illustrate a hardcoded-secret anti-pattern), `scripts/benchmark/cloud-init.yaml` sample configs, and `src/cmds/cloud/aws_cmd.rs` unit-test fixtures (fake AWS CloudWatch pagination tokens, in `#[cfg(test)]` blocks). No real credentials. [REDACTED - file:line + rule only, no secret values retained]

Rule breakdown: `stripe-access-token` (22), `generic-api-key` (16). Files: 10 historical `Security reports/*.md`, `SECURITY.md` (1), `scripts/benchmark/cloud-init.yaml` (2), `src/cmds/cloud/aws_cmd.rs` (5), `tests/guard_integration_test.rs` (2, test fixtures for the security-guard filter).

### TruffleHog (git, live verification)
0 verified secrets, 0 unverified secrets. Clean (20,138 chunks, 14.4 MB scanned).

### Trivy (filesystem, `--skip-dirs .claude/worktrees`)
0 vulnerabilities across `Cargo.lock` and all `tests/fixtures/**/pom.xml` fixtures. 0 secrets. Clean.

### OSV-Scanner (`Cargo.lock`)
Before fix: 1 vulnerability (RUSTSEC-2026-0285, rustls, CVSS 5.3 Medium, fixed in `0.23.45`).
After `cargo update -p rustls`: **No issues found.**

### Semgrep - secrets (`p/secrets`, full repo)
426 files, 45 rules, 0 findings.

### Semgrep - OWASP Top Ten (`p/owasp-top-ten`, py/js/ts/jsx/tsx/java/go/rb)
12 files (only 2 `.py`, 9 `.ts`, 1 `.rb` matched - rtk is a Rust project so this ruleset's language coverage is mostly N/A), 266 rules, 0 findings.

### Bandit (Python - `scripts/benchmark-sessions/`, `hooks/hermes/`)
9 Low-severity/High-confidence findings, all `B404`/`B603`/`B607` (subprocess module usage). Reviewed: all calls use list-form argv with fixed executable names (`tar`, etc.), no `shell=True`, no string-concatenated untrusted input. Standard subprocess-use advisories for benchmark/hook tooling, not exploitable as written. No fix applied.

### config-audit (Claude Code configuration - rtk-repo-scoped findings only)
Full scan also covers this machine's global `~/.claude` settings and installed plugins (out of scope for this repo, not actionable here - 124 of 126 total findings are global-config noise, e.g. third-party plugin hook registrations). Findings inside the rtk repo itself:
- HIGH x2 (`CLAUDE.md` + case-duplicate `claude.md` on this case-insensitive filesystem): "Suspicious command: curl to external URL" - false positive, matches doc/markdown link syntax, no executable curl instruction.
- MEDIUM x4 (same file pair): "instruction to skip verification" / "trust-all instruction" - false positive; this is the repo's own "Avoiding Rabbit Holes" maintainer guidance (bounding excessive verification loops), not attacker-injected content, and matches the pattern already reviewed in prior audits.

### skill-audit (all 12 `.claude/skills/*/SKILL.md` in repo)
| Skill | Score | Verdict | Driver | Verified |
|---|---|---|---|---|
| security-guardian | 100/100 | CRITICAL (flagged) | "destructive file deletion" / "format filesystem" pattern match | **False positive** - matches are inside Rust code-example strings illustrating attacker payloads (a comment showing what an injected destructive shell payload would look like, as something NOT to allow) and a blacklist-bypass example listing dangerous command names as strings; the skill is Read/Grep/Glob/Bash-only documentation, no destructive action is ever invoked |
| ship | 90/100 | CRITICAL (flagged) | Bash+Write+Edit tool access, `.env` keyword | **False positive** - tool access is inherent to its stated purpose (release automation: build/commit/push/tag); `.env` hit is a pre-commit checklist line ("No `.env` files committed"), not credential access; only outbound URL is `github.com/rtk-ai/rtk` (trusted, this project's own releases) |
| performance | 65/100 | HIGH (flagged) | `sudo dtrace` pattern match | **False positive** - documented macOS syscall-tracing command for startup-time profiling (dtrace requires sudo on macOS), prompt-only skill, no allowed-tools declared |
| rtk-tdd | 35/100 | MEDIUM | "sensitive system file" (`/etc/passwd`) | **False positive** - appears only in a path-traversal *test assertion example* (asserting a validator rejects a traversal path) |
| repo-recap | 30/100 | MEDIUM | tool access | Not investigated further - score driven by declared tools, no dangerous-pattern hits |
| code-simplifier, issue-triage | 25/100 each | MEDIUM | tool access | Not investigated further - no dangerous-pattern hits |
| design-patterns, pr-review, pr-triage, rtk-triage, tdd-rust | 5/100 each | LOW | - | Clean |

All CRITICAL/HIGH skill-audit verdicts were manually verified against source and are false positives from keyword-only pattern matching on documentation/example code, not executable instructions.

### mcps-audit
N/A - no `.mcp*`/`*.skill`/MCP config files found in the rtk repo itself.

### mcp-exfil-scan
Skipped - bundled script `SHA256SUMS` verification FAILED for `mcp-exfil-scan.sh` again this run (same as prior audits since at least 2026-07-07). Root cause is in the `claude-code-security-plugins` plugin cache, not the rtk codebase; not fixable from this repo. Recommend reinstalling that plugin.

## Cross-Tool Observations
- The 38 gitleaks findings and the config-audit HIGH/MEDIUM hits in `CLAUDE.md` reproduce the same false-positive set documented in `Security reports/security-audit-2026-07-07.md` and earlier reports - stable, expected noise from this repo's own security-audit history and test fixtures, not a regression.
- No tool (gitleaks, trufflehog, semgrep-secrets, trivy) found a real secret. No tool found a real SAST/injection vulnerability in application code.
- Only real, actionable finding across all tools: the OSV-Scanner rustls advisory, fixed this run.

## Coverage Gaps
- Business logic and IDOR-style checks are out of scope for this tool set.
- CodeQL skipped (no `.github/workflows/codeql.yml` configured in this repo).
- mcp-scan and skillspector LLM-assisted mode were not run (opt-in, require user consent for external data transmission; not requested for this autonomous run).
- mcp-exfil-scan skipped (bundled-script integrity check failure, see above).
- Global `~/.claude` config/plugin findings from config-audit are out of scope for this repo and not remediated here.

### APTS Audit Log
- **Log:** `/tmp/css-scan-20260923T021702Z.jsonl`
- **Tool runs recorded:** 9 (measured: 9, asserted: 0)
- **Standard:** OWASP APTS § Auditability
