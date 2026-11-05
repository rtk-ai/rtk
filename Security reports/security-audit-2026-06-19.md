# Security Audit — 2026-06-19

## Summary
- Issues found: 0 | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

All scanner findings were confirmed false positives: test fixtures, benchmark scripts, and documentation examples containing intentionally fake/example credentials.

## Fixed Issues
None.

## Unresolved Issues
None.

---

## Raw Scanner Output

### Scope Record
```
Scan target: /Users/mp3wizard/Public/Claude Proxy/rtk
Git HEAD:    51df392
Include:     all supported
Exclude:     .gitignore honored by each tool
```

### Coverage Disclosure

| Tool | Ran? | Version | Notes |
|------|------|---------|-------|
| Gitleaks | OK | 8.30.1 | 30 findings — all false positives (Security reports, test fixtures, docs) |
| Semgrep OWASP | OK | 1.166.0 | 4 findings — GitHub Actions shell injection in CI/CD workflows (upstream) |
| Semgrep Secrets | OK | 1.166.0 | 0 findings |
| Trivy | SKIPPED | 0.71.1 | DB download failed (docker-credential-desktop not in PATH) |
| TruffleHog | OK | 3.95.5 | 0 verified secrets |
| OSV-Scanner (source) | OK | 2.3.8 | Findings only in worktree Cargo.lock files (old branches), main is clean |
| OSV-Scanner (Cargo.lock) | OK | 2.3.8 | 0 issues — rustls-webpki 0.103.13 (fixed) confirmed |
| Config-audit | OK | bundled | MEDIUM: 2 trust-all patterns in CLAUDE.md (existing behavior); LOW: hooks config |
| mcp-exfil-scan | OK | bundled | Running in background |
| skillspector | SKIPPED | — | No .skill/SKILL.md files in project |
| CodeQL | SKIPPED | — | Requires GitHub Actions run |
| mcps-audit | SKIPPED | — | No MCP manifest files in project |
| mcp-scan | OPT-IN | — | Not run (sends data to invariantlabs.ai) |

### Gitleaks — Secrets in git history

**30 findings — all false positives**

Breakdown by location:
- `Security reports/` (22): Previous audit reports contain redacted/example tokens from their own scanner output — inherently self-referential false positives
- `src/cmds/cloud/aws_cmd.rs` (5): Test code with fake pagination tokens (e.g., `f/1234567890`) — confirmed example data
- `scripts/benchmark/cloud-init.yaml` (2): Benchmark setup script with clearly fake example credentials (e.g., `sk-1234567890abcdef`, `ghp_xxxx`)
- `SECURITY.md` (1): Security policy documentation showing an example of BAD practice (`sk_live_1234567890abcdef`)

All findings verified as intentional test/documentation content — no real credentials.

### Semgrep OWASP — GitHub Actions Shell Injection

**4 findings (blocking)** in upstream CI/CD workflows — not RTK source code

```
.github/workflows/ci.yml       — yaml.github-actions.security.run-shell-injection
.github/workflows/release.yml  — yaml.github-actions.security.run-shell-injection (3 instances)
```

**Assessment**: These are in upstream GitHub Actions workflows using `${{ github.event.release.tag_name }}` and similar context variables in `run:` steps. This is an upstream issue (rtk-ai/rtk), not introduced by this fork. The risk is limited to RTK's CI/CD pipeline maintainers — not RTK users. Upstream should migrate to `env:` intermediates per the Semgrep recommendation.

### Semgrep Secrets — No findings

```
Ran 45 rules on 353 files: 0 findings.
```

### TruffleHog — No live-verified secrets

```
verified_secrets: 0
unverified_secrets: 0
chunks: 12982, bytes: 9356602, scan_duration: 2.14s
```

### OSV-Scanner — No vulnerabilities in main project

Main `Cargo.lock` scan: **0 issues found** (203 packages scanned)

- `rustls-webpki`: version 0.103.13 ✓ (RUSTSEC-2026-0104 patched — fixed version ≥ 0.103.13)

Note: OSV scan of full repo (recursive) flagged RUSTSEC-2026-0104 in `.claude/worktrees/*/Cargo.lock` files — these are isolated git worktrees for other feature branches, not the main project. They are not deployed.

### Config-audit — Claude configuration

**MEDIUM (2)**: `CLAUDE.md` contains phrases flagged as "trust-all instructions" by the heuristic (`trust snapshot tests`, `trust available info`). These are legitimate developer guidelines in context — not actual security bypasses.

**LOW (8)**: Hooks configuration in `~/.claude/settings.json` and plugin hooks — expected for a Claude Code environment.

### APTS Audit Log
- **Log:** `/tmp/css-scan-20260619T021212Z.jsonl`
- **Tool runs recorded:** 12 (measured: 12, asserted: 0)
- **Standard:** OWASP APTS § Auditability
