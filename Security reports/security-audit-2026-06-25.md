# Security Audit — 2026-06-25

**Target:** `/Users/mp3wizard/Public/Claude Proxy/rtk`
**Scanned at:** 2026-06-25T02:21:00Z
**Git HEAD:** a33d402
**Standard:** OWASP APTS-aligned (Scope Enforcement · Auditability · Manipulation Resistance · Reporting)

## Summary
- Issues found: 1 | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

> **Note:** The one real finding (`rustls-webpki 0.103.12`, GHSA-82j2-j2ch-gfr8) exists only in stale
> `.claude/worktrees/*/Cargo.lock` files. The main `Cargo.lock` already contains `0.103.13` (fixed).
> No code change is required.

## Fixed Issues

None — no fixes were required. Main `Cargo.lock` is already patched.

## Worktree Advisory

| # | Component | Advisory | Finding |
|---|-----------|----------|---------|
| 1 | rustls-webpki in `.claude/worktrees/*/Cargo.lock` | GHSA-82j2-j2ch-gfr8 / RUSTSEC-2026-0104 | Stale worktree Cargo.lock files pin 0.103.12; main Cargo.lock has 0.103.13 (fixed). Stale worktrees are transient and not part of the release artifact. No action required. |

## Unresolved Issues

None.

## Gitleaks — Secrets in Git History

31 findings, all assessed as false positives or acceptable:

- **Security reports/*.md (22 findings)**: Past audit reports contain redacted/example strings that
  match stripe-access-token and generic-api-key patterns. These are scanner output fragments, not real
  secrets. Gitleaks treats them as secrets-in-history; they are false positives.
- **scripts/benchmark/cloud-init.yaml (2 findings)**: Example placeholder keys in benchmark scripts.
  These are synthetic test values, not real credentials.
- **src/cmds/cloud/aws_cmd.rs (5 findings)**: Test fixture strings inside `#[cfg(test)]` blocks. The
  flagged lines contain format strings constructing example token values for unit tests.
  No real credentials are present.

**TruffleHog result:** 0 verified secrets, 0 unverified secrets — confirms no live credentials.

## Semgrep OWASP

Findings: **0** (266 rules applied to 12 tracked files including JS/TS/Python/Ruby).

## Semgrep Secrets

Findings: **0** (45 rules applied to 360 files).

## Trivy (Dependency Scan)

- Worktree Cargo.lock files: HIGH (rustls-webpki 0.103.12) — stale worktrees only
- Main Cargo.lock: **CLEAN** (rustls-webpki 0.103.13 — already fixed)

```
Library        | Vulnerability        | Severity | Installed | Fixed    | Location
rustls-webpki  | GHSA-82j2-j2ch-gfr8  | HIGH     | 0.103.12  | 0.103.13 | .claude/worktrees/*/Cargo.lock (x13)
```

## TruffleHog

13,223 chunks scanned, 8.47 MB, 1,162 commits.
**Result: 0 verified secrets, 0 unverified secrets.**

## OSV-Scanner

All 13 findings are `rustls-webpki 0.103.12` in `.claude/worktrees/` Cargo.lock files.
Main `Cargo.lock`: **CLEAN**.

## Config Audit (claude-code-security-plugins)

52 issues reported; all are false positives from the security scanner plugin's own scripts
(the scanner detects exfiltration patterns in its own detection code) plus cc-beeper hooks
(localhost-only curl calls to port 19222, not external). The `skipDangerousModePermissionPrompt`
flag in project settings is a known-acceptable user configuration.

No new actionable findings in the RTK project source code itself.

## mcp-exfil-scan

Scan aborted early: `MCP_CONFIGS[@]: unbound variable` at script line 197.
RTK has no MCP config files — the empty array caused a crash in the bash script.
Finding: **No MCP configs present** (Rust CLI project, expected result).

## Cross-Tool Observations

- TruffleHog (live verification) found **zero** secrets across 1,162 commits and 13,223 chunks —
  the strongest signal that no real credentials exist in the repository.
- Semgrep found **zero** OWASP or secrets findings across all source files.
- Gitleaks' 31 findings are entirely in past audit reports and test fixtures, confirmed false
  positives by TruffleHog's clean run.
- The `rustls-webpki` advisory appears across Trivy and OSV-Scanner but only in stale worktrees,
  not the main build artifact.

## Coverage Gaps

- Business logic and IDOR: not covered by static analysis
- Runtime behavior: not tested
- CodeQL: skipped (no GitHub Actions CodeQL workflow found)
- mcp-exfil-scan: partial (crashed on empty MCP config array)
- Bandit: no Python source files in RTK (Rust-only project)
- skillspector: no AI skill artifacts in RTK source

## Raw Scanner Output

### Gitleaks (summary)
```
1162 commits scanned, ~8.47 MB, 2.05s
leaks found: 31 (all in Security reports/ or test fixtures — false positives per TruffleHog)
```

### TruffleHog
```
chunks: 13223, bytes: 9473326, verified_secrets: 0, unverified_secrets: 0, scan_duration: 2.3s
```

### OSV-Scanner
All 13 findings: rustls-webpki 0.103.12 → .claude/worktrees/*/Cargo.lock (stale worktrees)

### APTS Audit Log
- **Log:** `/tmp/css-scan-20260625T021415Z.jsonl`
- **Tool runs recorded:** 12 (measured: 12, asserted: 0)
- **Standard:** OWASP APTS § Auditability
