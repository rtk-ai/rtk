# Security Audit — 2026-05-31

**Target:** `/Users/mp3wizard/Public/Claude Proxy/rtk`
**Scanned at:** 2026-05-31T02:26:19Z
**Git HEAD:** 30c4384
**Standard:** OWASP APTS-aligned (Scope Enforcement · Auditability · Manipulation Resistance · Reporting)

## Summary
- Issues found: 1 actionable | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

> **Main codebase status: CLEAN.** The one HIGH finding (rustls-webpki GHSA-82j2-j2ch-gfr8) was already patched in the main `Cargo.lock` (v0.103.13 pinned via `=0.103.13` in `Cargo.toml`). All scanner alerts came from stale `.claude/worktrees/*/Cargo.lock` files — development artifacts, not shipped code. No code changes required.

## Scope Record
```
Scan target: /Users/mp3wizard/Public/Claude Proxy/rtk
Git HEAD:    30c4384
Include:     all supported
Exclude:     .gitignore honored by each tool
```

## Coverage Disclosure (APTS § Reporting)
| Tool | Ran? | Version | Findings | Skipped reason |
|------|------|---------|----------|----------------|
| Gitleaks | OK | 8.30.1 | 17 FP (placeholder examples in security report docs) | — |
| Bandit | OK | 1.9.4 | 9 LOW (subprocess use in benchmark scripts) | — |
| Semgrep | OK | 1.157.0 | 0 | — |
| Trivy | OK | 0.69.3 | 1 HIGH (worktree lockfiles only — main already fixed) | — |
| TruffleHog | OK | 3.94.2 | 0 verified, 0 unverified | — |
| CodeQL | SKIPPED | — | — | No `.github/workflows/codeql.yml` |
| mcps-audit | SKIPPED | — | — | npx blocked by auto-mode classifier |
| OSV-Scanner | OK | 2.3.5 | 13 HIGH (all from worktree lockfiles — main already fixed) | — |
| mcp-scan | OPT-IN | — | — | Not opted in (sends data to invariantlabs.ai) |
| security-audit (config-audit.py) | OK | bundled | 43 total (5 CRITICAL/12 HIGH: all FP — security/audit tool self-references) | — |
| skill-security-auditor | OK | bundled | Low/Medium (no critical in RTK project skills) | — |
| mcp-exfil-scan | OK | bundled | 11 (all in global ~/.claude/skills — not RTK codebase) | — |

## Gitleaks — Secrets Detection

**Summary:** 17 findings — all false positives

All 17 findings are documentation/example placeholder values (`msk_live_1234567890abcdef`, `msk-1234567890abcdef`, `ghp_xxx`) found in:
- `Security reports/security-audit-*.md` — prior audit reports citing these as example FPs
- `scripts/benchmark/cloud-init.yaml` — benchmark placeholder `API_KEY=msk-1234567890abcdef`
- `src/cmds/cloud/aws_cmd.rs` — CloudWatch pagination tokens (`f/...`, `b/...`) in test fixtures

No real secrets found. 1,046 commits and 7.89 MB scanned in 1.2s.

## Bandit — Python SAST

**Summary:** 9 LOW issues, 0 MEDIUM, 0 HIGH

All findings are in `scripts/benchmark-sessions/lib/runner.py` and `hooks/hermes/` — benchmark and hook helper scripts:
- B404 (subprocess import): subprocess module used in benchmark runner — expected, not user-facing
- B607 (partial path): `tar`, `git` called by name without full path — standard benchmark tooling  
- B603 (subprocess without shell=True): subprocess with list args — this is actually the SAFE pattern

No production RTK source code affected.

## Semgrep — Multi-language SAST

**Summary:** 0 findings across all rulesets

- OWASP Top 10: 0 findings (12 files, 266 rules)
- Python: 0 findings (2 files, 151 rules)
- TypeScript: 0 findings (9 files, 74 rules)
- Secrets: 0 findings (341 files, 45 rules)

## Trivy — Dependency Vulnerabilities

**Summary:** 1 HIGH — rustls-webpki GHSA-82j2-j2ch-gfr8 — **already fixed in main codebase**

| Library | Advisory | Severity | Status | Installed | Fixed | Source |
|---------|----------|----------|--------|-----------|-------|--------|
| rustls-webpki | GHSA-82j2-j2ch-gfr8 | HIGH | **fixed in main** | 0.103.12 (worktrees only) | 0.103.13 | `.claude/worktrees/*/Cargo.lock` |

**Verification:** Main `Cargo.lock` contains `rustls-webpki = 0.103.13`. `Cargo.toml` pins `[dependencies.rustls-webpki] version = "=0.103.13"`. The 0.103.12 findings are from 13 stale git worktrees used for parallel development — these are not shipped code.

**Advisory detail:** DoS via panic on malformed CRL BIT STRING (CVSS 7.5). Not exploitable in production since main codebase is patched.

## TruffleHog — Secret Scanning with Live Verification

**Summary:** 0 verified secrets, 0 unverified secrets

11,985 chunks scanned, 8.5 MB in 1.79s. No secrets detected.

## OSV-Scanner — Software Composition Analysis

**Summary:** 13 HIGH (CVSS 7.5) — all from worktree lockfiles, main project clean

All 13 findings are RUSTSEC-2026-0104 / GHSA-82j2-j2ch-gfr8 (rustls-webpki 0.103.12) in `.claude/worktrees/` Cargo.lock files. Main project `Cargo.lock` already at 0.103.13. No action required.

## Security Config Audit (config-audit.py)

**Summary:** 43 findings — analysis below

| Severity | Count | Assessment |
|----------|-------|------------|
| CRITICAL | 5 | All FP: security scanner skill self-references (mcp-exfil-scan.sh, skill-audit.sh scan for these patterns by design) |
| HIGH | 12 | 7 FP: `cc-beeper` localhost-only webhook hooks in user settings; 2 FP: CLAUDE.md curl references in documentation examples; 2 FP: hook-development example script that checks for dangerous commands by listing them; 1 FP: `optimize` skill uses netcat keyword in documentation |
| MEDIUM | 21 | Hook broad-matchers (intentional), skipDangerousModePermissionPrompt (user preference), skill sensitive-file references (browser testing, notebooklm — expected) |
| LOW | 5 | Hooks config present (informational) |

**Key finding — cc-beeper hooks:** The curl-to-external-URL HIGH alerts are for `http://localhost:${PORT}/hook` — a local notification system (`cc-beeper`). These are legitimate user-configured hooks, not exfiltration.

**CLAUDE.md findings:** "Suspicious instruction to skip verification" and "trust-all instruction" are FPs — the text refers to avoiding rabbit holes in testing (e.g., "trust snapshot tests") which is project-specific development guidance, not a security bypass instruction.

## MCP Exfil Scan

**Summary:** 11 findings — all in global `~/.claude/skills/`, none in RTK codebase

| Severity | Finding | Assessment |
|----------|---------|------------|
| CRITICAL | security-audit/SKILL.md — exfiltration instruction pattern | FP: security scanner lists what it detects |
| CRITICAL | frontend-design/SKILL.md — exfiltration instruction | FP: design skill mentions screenshot/export |
| HIGH | skill-security-auditor — Read+WebFetch+Bash chain | FP: security auditor needs these tools by design |
| HIGH | atlas-cloud — URL shortener pattern, env leak | FP: AI API calls use proper SDK patterns |
| MEDIUM | playwright-cli, pyright, vtsls — no source attribution | Informational — bundled skills |

All findings are from global user skills, not RTK codebase files.

## Cross-Tool Observations

- **rustls-webpki** consistently flagged by Trivy and OSV-Scanner, confirming GHSA-82j2-j2ch-gfr8 presence — but exclusively in worktree Cargo.lock files. Main codebase already patched. No action required.
- **Gitleaks false positives** in security report markdown files are a recurring pattern from prior audits. These are known FPs and can be suppressed with a `.gitleaksignore`.
- **No real secrets** found by any tool (Gitleaks, TruffleHog, Semgrep secrets).
- **No SAST findings** in core Rust source code.

## Coverage Gaps

- Business logic, IDOR, and runtime behavior not covered by static analysis.
- CodeQL not configured (no GitHub Actions workflow).
- mcps-audit skipped (npx classifier block — transient).
- mcp-scan not opted in (privacy: sends data to invariantlabs.ai).

## Fixed Issues

None — main codebase was already patched (rustls-webpki 0.103.13).

## Unresolved Issues

None.

## Raw Scanner Output

### Gitleaks
- 17 findings (all FP): placeholder secrets in documentation files and test fixtures
- 1,046 commits scanned, 7.89 MB in 1.2s

### Bandit
- 9 LOW (B404, B607, B603): subprocess use in benchmark scripts
- 467 lines of Python code scanned

### Semgrep
- 0 findings: OWASP, Python, TypeScript, Secrets rulesets all clean

### Trivy
- 1 HIGH per lockfile: rustls-webpki 0.103.12 → 0.103.13 (in worktrees only)
- Main Cargo.lock: rustls-webpki 0.103.13 ✅

### TruffleHog
- 0 verified, 0 unverified secrets
- 11,985 chunks, 8.5 MB scanned

### OSV-Scanner
- 13 HIGH: RUSTSEC-2026-0104 / GHSA-82j2-j2ch-gfr8 in worktree lockfiles
- Main project Cargo.lock: clean

### APTS Audit Log
### APTS Audit Log
- **Log:** `/tmp/css-scan-20260531T022619Z.jsonl`
- **Tool runs recorded:** 9
- **Standard:** OWASP APTS § Auditability
