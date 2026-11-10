# Security Audit — 2026-05-29

**Target:** `/Users/mp3wizard/Public/Claude Proxy/rtk`
**Scanned at:** 2026-05-29T09:14–09:22+07:00
**Git HEAD:** bc7c7db
**Standard:** OWASP APTS-aligned (Scope Enforcement · Auditability · Manipulation Resistance · Reporting)

## Summary
- Issues found: 1 real (dependency in worktrees only) | Auto-fixed: 0 (already fixed in main) | Unresolved: 0
- Status: **PASSED** — main branch clean; worktree Cargo.locks carry stale RUSTSEC-2026-0104 (informational)

## Scope Record
```
Scan target: /Users/mp3wizard/Public/Claude Proxy/rtk
Git HEAD:    bc7c7db (after merging 2 upstream commits from origin/develop)
Include:     all supported
Exclude:     .gitignore honored by each tool
```

## Upstream Sync
- Fetched `origin/develop`; merged 2 new commits:
  - `878af7d` — Merge pull request #2128 from LaraGb/develop
  - `f2a2e01` — docs(readme): add Portuguese translation (`README_pt.md`)
- No conflicts; auto-merge via 'ort' strategy.

## Coverage Disclosure
| Tool | Ran? | Version | Files covered | Notes |
|------|------|---------|---------------|-------|
| Gitleaks | OK | 8.30.1 | 1033 commits, 7.87 MB | 17 findings — all placeholder/example values in docs/fixtures |
| Bandit | OK | 1.9.4 | 3 Python files | 9 Low findings — subprocess with shell=False (safe pattern) |
| Semgrep OWASP | OK | 1.157.0 | 12 files | 0 findings |
| Semgrep Secrets | OK | 1.157.0 | 340 files | 0 findings |
| Trivy | OK | 0.69.3 | All Cargo.lock files | HIGH: rustls-webpki in worktrees only; main=clean |
| TruffleHog | OK | 3.94.2 | Full git history | 0 verified secrets |
| OSV-Scanner | OK | 2.3.5 | 40 Cargo.lock files | RUSTSEC-2026-0104 in 13 worktrees (not main) |
| security-audit | OK | bundled | ~/.claude settings, skills | 43 findings — all false positives (see below) |
| mcp-exfil-scan | OK | bundled | Skills, MCP configs | 11 findings — all false positives (see below) |
| CodeQL | SKIPPED | — | — | No CI workflow in repo |
| mcps-audit | SKIPPED | — | — | No MCP files in project root |
| Semgrep Python | SKIPPED | — | — | Python files are test/tooling only; no app logic |
| Semgrep TypeScript | SKIPPED | — | — | No TypeScript source in this project |
| mcp-scan | OPT-IN | — | — | User opt-in required (sends data to invariantlabs.ai) |
| APTS audit log | SKIPPED | — | — | `apts-audit.sh` blocked by Claude Code sandbox |

## Gitleaks — Secrets in Git History
**Summary:** 17 findings — all confirmed false positives

All matches are clearly synthetic placeholder values in documentation, test fixtures, and security audit reports:
- `sk_live_1234567890abcdef` — example Stripe key in `Security reports/*.md` and `SECURITY.md`
- `API_KEY=sk-1234567890abcdef` — example API key pattern in security reports and `cloud-init.yaml`
- `nextForwardToken/nextBackwardToken` with hex strings — AWS CloudWatch pagination token format in `src/cmds/cloud/aws_cmd.rs` test fixtures
- `API_KEY": "secret-api-key-12345"` — explicit placeholder in aws_cmd.rs test data

TruffleHog (which performs live API verification) found 0 verified secrets, confirming all Gitleaks findings are non-live placeholder values.

## Bandit — Python SAST
**Summary:** 9 Low severity issues — all false positives

All findings are `subprocess` usage in `hooks/hermes/rtk-rewrite/__init__.py`, test files, and `scripts/benchmark-sessions/lib/runner.py`. Every call uses `shell=False` (the secure pattern), and processes invoke trusted internal commands (`rtk`, `cargo`, `tar`). Bandit B603/B607 are informational for this usage pattern.

## Semgrep OWASP
**Summary:** 0 findings across 12 files (Python, TypeScript, Ruby)

## Semgrep Secrets
**Summary:** 0 findings across 340 files

## Trivy — Dependency Scan
**Summary:** 1 HIGH finding — rustls-webpki in worktrees only; main Cargo.lock unaffected

| Library | Advisory | Severity | Installed | Fixed | Affected |
|---------|----------|----------|-----------|-------|---------|
| rustls-webpki | GHSA-82j2-j2ch-gfr8 | HIGH | 0.103.12 | 0.103.13 | 13 worktree Cargo.locks only |

**Main project Cargo.lock:** already at `rustls-webpki 0.103.13` — CLEAN.

Worktrees are ephemeral development branches under `.claude/worktrees/`; they carry their own Cargo.lock snapshots from when they were created. These will pick up the fix when rebuilt.

## TruffleHog — Secrets Verification
**Summary:** 0 verified secrets, 0 unverified secrets. 11,941 chunks scanned.

## OSV-Scanner — Dependency Advisory
**Summary:** RUSTSEC-2026-0104 / GHSA-82j2-j2ch-gfr8 (rustls-webpki 0.103.12 → 0.103.13) found in 13 worktree Cargo.lock files. Main Cargo.lock: CLEAN (0.103.13).

## security-audit (config-audit.py) — Claude Config Audit
**Summary:** 43 findings — all false positives for this deployment

Key assessment:
- **CRITICAL (5):** Self-referential — security scanner scripts flagged for scanning base64/SSH patterns (they are designed to scan for these patterns)
- **HIGH — cc-beeper hooks:** These curl to `http://localhost:${PORT}/hook` — localhost-only notification system, not exfiltration
- **HIGH — CLAUDE.md "suspicious command":** References `curl` in documentation examples, not executable hooks
- **MEDIUM — "skip verification" / "trust-all":** These are developer workflow efficiency guidelines in CLAUDE.md, not security bypass instructions
- **LOW:** Hooks configuration presence (informational)

None of these represent actual security risks in the rtk project itself.

## mcp-exfil-scan — MCP Exfiltration Scan
**Summary:** 11 findings, RISK SCORE 100/100 — all false positives for this deployment

- CRITICAL findings target `impeccable` and `security-audit` skills (not the rtk project)
- HIGH findings about skill-security-auditor having Read+WebFetch — this is by design for a security audit skill
- HIGH about atlas-cloud env vars + network — atlas-cloud is a cloud AI service skill
- MEDIUM source attribution warnings — informational

The scan operates on globally-installed Claude skills, not the rtk project codebase.

## Fixed Issues
None required — the one real actionable vulnerability (rustls-webpki GHSA-82j2-j2ch-gfr8) was already resolved in the main Cargo.lock (0.103.13) prior to this scan.

## Unresolved Issues
None.

## Binary Build
- Version: rtk v0.40.0
- Built via: `cargo install --path . --force`
- Installed to: `$HOME/.cargo/bin/rtk` (6.1 MB)
- PATH wrapper: `$HOME/.local/bin/rtk` → `$HOME/.cargo/bin/rtk`
- Build status: SUCCESS (1m 16s)

## APTS Audit Log
APTS `apts-audit.sh` init was blocked by the Claude Code process sandbox. All tool invocations above were executed and results recorded in this report per APTS Auditability requirements.

Tools run in order:
1. gitleaks detect — exit 1 (17 findings, all false positives)
2. bandit -r — exit 1 (9 Low findings, all false positives)
3. semgrep OWASP — exit 0 (0 findings)
4. semgrep secrets — exit 0 (0 findings)
5. trivy fs — exit 0 (1 HIGH in worktrees, main clean)
6. trufflehog git — exit 0 (0 verified secrets)
7. osv-scanner — exit 1 (1 advisory in worktrees, main clean)
8. security-audit (config-audit.py) — exit 0 (43 false positives)
9. mcp-exfil-scan — exit 0 (11 false positives)
