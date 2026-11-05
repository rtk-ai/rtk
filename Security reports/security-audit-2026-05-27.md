# Security Audit — 2026-05-27

**Target:** `/Users/mp3wizard/Public/Claude Proxy/rtk`  **Scanned at:** 2026-05-27T09:13:47Z  **Git HEAD:** 431788b
**Standard:** OWASP APTS-aligned (Scope Enforcement · Auditability · Manipulation Resistance · Reporting)

## Summary
- Issues found: 1 | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

> **Note on "Issues found":** The single substantive issue — `rustls-webpki` GHSA-82j2-j2ch-gfr8 — was found by Trivy and OSV-Scanner **exclusively in worktree Cargo.lock files** (`.claude/worktrees/*/Cargo.lock`). The **main project** `Cargo.lock` already contains the fixed version `0.103.13` (pulled in via the upstream merge). No code change was required. All other findings (Gitleaks, config-audit, mcp-exfil-scan) are false positives — see details below.

## Scope Record
```
Scan target: /Users/mp3wizard/Public/Claude Proxy/rtk
Git HEAD:    431788b
Include:     all supported
Exclude:     .gitignore honored by each tool
```

## Coverage Disclosure (APTS § Reporting)
| Tool | Ran? | Version | Files covered | Skipped reason |
|------|------|---------|---------------|----------------|
| Gitleaks | OK | 8.30.1 | 1030 commits, ~7.85 MB | — |
| Bandit | N/A | 1.9.4 | — | No .py files in project |
| Semgrep (OWASP) | OK | latest | 12 files | — |
| Semgrep (secrets) | OK | latest | 338 files | — |
| Trivy | OK | 0.69.3 | Cargo.lock + worktrees | — |
| TruffleHog | OK | 3.94.2 | 11928 chunks, ~8.5 MB | — |
| CodeQL | SKIPPED | — | — | Not triggered (no new workflow) |
| OSV-Scanner | OK | 2.3.5 | 203 packages per Cargo.lock | — |
| mcp-scan | OPT-IN | — | — | Not consented |
| security-audit (config-audit.py) | OK | bundled | ~/.claude + project CLAUDE.md | — |
| skill-security-auditor | SKIPPED | — | — | No SKILL.md in project |
| mcp-exfil-scan | OK | bundled | ~/.claude/skills + project | — |

## Gitleaks — Secrets in git history

**Summary:** 13 findings — all **false positives** in test fixtures and documentation

| # | File | RuleID | Verdict |
|---|------|--------|---------|
| 1 | `Security reports/security-audit-2026-05-22.md:36` | generic-api-key | FP — content from prior audit report |
| 2-4 | `Security reports/security-audit-2026-04-1*.md` | stripe-access-token | FP — example tokens in prior audit reports |
| 5 | `Security reports/2026-04-16-security-report.md:64` | stripe-access-token | FP — example in prior audit report |
| 6-7 | `scripts/benchmark/cloud-init.yaml:282,613` | generic-api-key | FP — `sk-1234567890abcdef` placeholder in benchmark script |
| 8-12 | `src/cmds/cloud/aws_cmd.rs:1878,1879,1923,2035` | generic-api-key | FP — test fixtures inside `#[cfg(test)]` blocks |
| 13 | `SECURITY.md:151` | stripe-access-token | FP — `sk_live_1234567890abcdef` in SECURITY.md as example bad code |

**Analysis:** `SECURITY.md` uses `const API_KEY: &str = "sk_live_1234567890abcdef"` as an example of hardcoded secrets (explicitly illustrating bad practice). `aws_cmd.rs` lines are within `#[cfg(test)]` test assertions. Benchmark cloud-init.yaml uses obvious placeholder values. No real credential exposure.

## Semgrep — OWASP Top 10 + Secrets

**Summary:** 0 findings

Scanned 12 files (OWASP, 266 rules) and 338 files (secrets, 45 rules). No findings.

## Trivy — Dependency Vulnerabilities

**Summary:** 1 finding (HIGH) — worktrees only, main project already patched

| Library | CVE/Advisory | Severity | Installed | Fixed | Location |
|---------|-------------|----------|-----------|-------|----------|
| rustls-webpki | GHSA-82j2-j2ch-gfr8 / RUSTSEC-2026-0104 | HIGH (CVSS 7.5) | 0.103.12 | 0.103.13 | `.claude/worktrees/*/Cargo.lock` (13 worktrees) |

**Main project status:** `Cargo.lock` already contains `rustls-webpki = 0.103.13` (fixed). The worktree Cargo.lock files are stale copies from in-progress work branches and do not affect the installed binary.

**Advisory:** Denial of service via panic on malformed CRL BIT STRING in rustls-webpki. Affects TLS certificate validation. Fixed in 0.103.13.

## TruffleHog — Live-Verified Secrets

**Summary:** 0 verified secrets, 0 unverified secrets

Scanned 11,928 chunks, ~8.5 MB. Clean.

## OSV-Scanner — Software Composition Analysis

**Summary:** 13 entries (all same advisory, all worktrees only)

All 13 findings are `rustls-webpki 0.103.12` (GHSA-82j2-j2ch-gfr8) in worktree Cargo.lock files. The main `Cargo.lock` (203 packages) is clean. Confirmed fixed version 0.103.13 in main project.

## Config Audit (config-audit.py) — Claude Configuration

**Summary:** 9 MEDIUM, 5 LOW — all informational / false positives

| Severity | Finding | Verdict |
|----------|---------|---------|
| MEDIUM | `plugin:openai-codex/hooks.json` — broad-matcher hooks (SessionStart, SessionEnd, Stop) | Informational — expected plugin lifecycle hooks |
| MEDIUM | `plugin:addy-agent-skills/hooks.json` — broad-matcher SessionStart | Informational — expected |
| MEDIUM | `plugin:pordee/plugin.json` — broad-matcher hooks (SessionStart, UserPromptSubmit) | Informational — expected |
| MEDIUM (×2) | `CLAUDE.md` + `claude.md` — "instruction to skip verification" / "trust-all instruction" | FP — project efficiency guidance, not security bypass |
| LOW (×5) | Various hooks configurations | Informational |

**CLAUDE.md findings clarification:** The "skip verification" finding matches the project guidance "Do not make excessive operations to verify external APIs" — this is a developer efficiency rule, not a security bypass instruction.

## MCP Exfiltration Scan

**Summary:** 11 findings — all in `~/.claude/skills/`, not in RTK project; all false positives

| Severity | Finding | Verdict |
|----------|---------|---------|
| CRITICAL | `impeccable/SKILL.md` — "exfiltration instruction pattern" | FP — pattern matched the word "exfiltration" in a defensive context |
| CRITICAL | `security-audit/SKILL.md` — same | FP — security audit skill describes scanning for exfiltration |
| HIGH | `skill-security-auditor/SKILL.md` — Read+WebFetch chain | FP — by design for a security auditing skill |
| HIGH | `skill-security-auditor/SKILL.md` — Bash+WebFetch | FP — same |
| HIGH | `atlas-cloud/SKILL.md` — URL shortener | FP — OpenAI API endpoint, not a URL shortener |
| HIGH (×2) | `atlas-cloud/SKILL.md`, `skill-security-auditor/SKILL.md` — env vars + network | FP — expected for API/security skills |
| MEDIUM (×4) | Various skills — no source attribution | Informational |

**Scope note:** All findings are in `~/.claude/skills/` (user's global skill directory), not in the RTK project codebase. RTK itself has no MCP servers or skills.

## Cross-Tool Observations

- **rustls-webpki GHSA-82j2-j2ch-gfr8** found by both Trivy and OSV-Scanner — corroborated. Main project already fixed at 0.103.13. Only worktree stale lockfiles are affected.
- **No cross-tool secret confirmation** — Gitleaks found 13 matches, TruffleHog verified 0. All Gitleaks findings are test fixtures/documentation examples.
- Semgrep, TruffleHog scan clean. OWASP rules found no vulnerabilities in Rust or JS/TS code.

## Coverage Gaps
- Business logic and IDOR issues not covered by static analysis
- Runtime behavior not verified
- CodeQL skipped (no workflow trigger)
- mcp-scan (opt-in) not run

### APTS Audit Log
- **Log:** `/tmp/css-scan-20260527T021347Z.jsonl`
- **Tool runs recorded:** 9
- **Standard:** OWASP APTS § Auditability
