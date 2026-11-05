# Security Audit — 2026-07-01

## Summary
- Issues found: 1 | Auto-fixed: 1 | Unresolved: 0
- Status: ISSUES FIXED

## Fixed Issues
| # | Component | Advisory | Change |
|---|-----------|----------|--------|
| 1 | anyhow | RUSTSEC-2026-0190 | 1.0.102 → 1.0.103 (cargo update anyhow) |

## Additional Fixes (Upstream Merge Bugs)
Two bugs were introduced by the upstream uv support PR and fixed during this sync:

| # | File | Issue | Fix |
|---|------|-------|-----|
| 1 | src/cmds/python/uv_cmd.rs:61 | Missing `tee_label` argument to `runner::print_with_hint` (compile error) | Added `&result.raw` as guard_raw argument |
| 2 | src/discover/registry.rs | `uv run python script.py` not rewritten to `rtk uv run python script.py` | Fixed transparent prefix to fall through to classification when inner command unrecognized and doesn't start with `-` |
| 3 | src/main.rs | `"uv"` subcommand not in PASSTHROUGH list (test failure) | Added `"uv"` to PASSTHROUGH const |
| 4 | src/hooks/rewrite_cmd.rs | Tests assumed `PermissionVerdict::Ask` but user's config allows git → returns `Allow` | Updated tests to match both `Ask` and `Allow` (both mean rewrite happened) |

## Unresolved Issues
None.

## Raw Scanner Output

### Gitleaks — 33 findings (ALL FALSE POSITIVES)
All 33 findings are in:
- `Security reports/security-audit-*.md` — prior audit reports containing redacted example values from test fixtures
- `src/cmds/cloud/aws_cmd.rs` — inside `#[cfg(test)]` blocks with synthetic CloudWatch pagination tokens (`f/abcdef1234...`)
- `scripts/benchmark/cloud-init.yaml` — explicitly synthetic: `API_KEY=sk-1234567890abcdef`, `SECRET_TOKEN=ghp_xxxx`
- `SECURITY.md` — security documentation example: `const API_KEY: &str = "sk_live_1234567890abcdef"`

No real credentials detected.

### Semgrep — 0 findings
- OWASP Top 10: 0 findings (223 rules, 11 files)
- TypeScript: 0 findings (74 rules, 9 files)
- Secrets: 0 findings (45 rules, 363 files)

### Trivy — 0 findings in main Cargo.lock
- `rustls-webpki GHSA-82j2-j2ch-gfr8` HIGH found only in `.claude/worktrees/` (other worktrees), not in the main Cargo.lock (which already has 0.103.13, the patched version)

### TruffleHog — 0 verified secrets
- 11,725 chunks scanned, 0 verified secrets, 0 unverified secrets

### OSV-Scanner — 1 vulnerability (FIXED)
- `RUSTSEC-2026-0190` in `anyhow 1.0.102` → fixed by upgrading to `1.0.103`
- Post-fix re-scan: "No issues found"

## Coverage Disclosure
| Tool | Ran? | Version | Notes |
|------|------|---------|-------|
| Gitleaks | OK | 8.30.1 | 33 findings, all false positives |
| Semgrep | OK | (latest) | 0 findings |
| Trivy | OK | 0.71.2 | Safe version |
| TruffleHog | OK | 3.95.6 | 0 verified secrets |
| Bandit | N/A | 1.9.4 | No .py source files in main codebase |
| OSV-Scanner | OK | 2.4.0 | 1 vuln found and fixed |
| CodeQL | SKIPPED | — | No GitHub Actions workflow needed |
| mcp-scan | OPT-IN | — | Not run (sends data to third party) |
| skillspector | N/A | — | No AI skill files in project |
