# Security Audit — 2026-06-08

## Summary
- Issues found: 0 (actionable) | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

All scanner findings are either in stale worktree `Cargo.lock` files (not the main project), synthetic test fixtures, past audit reports with example credentials, or CLAUDE.md anti-rabbit-hole instructions (false positives). The main project codebase and `Cargo.lock` are clean.

## Fixed Issues
None — no actionable vulnerabilities found in the main codebase.

## Raw Scanner Output

### Gitleaks v8.30.1
- 1088 commits scanned (~8.06 MB in 1.65s)
- 23 raw findings — all false positives:
  - 15 findings in `Security reports/` (prior audit reports containing synthetic/example API keys)
  - 5 findings in `src/cmds/cloud/aws_cmd.rs` test fixtures (e.g. `"API_KEY": "secret-api-key-12345"`)
  - 2 findings in `scripts/benchmark/cloud-init.yaml` (test benchmarking config)
  - 1 finding in `SECURITY.md` (documentation example: `sk_live_1234567890abcdef`)
- No real credentials found. TruffleHog independently confirmed: 0 verified secrets, 0 unverified secrets.

### Semgrep v1.x (Secrets ruleset — p/secrets)
```
Findings: 0 (0 blocking)
Rules run: 45
Targets scanned: 346
Parsed lines: ~100.0%
```

### Semgrep (OWASP Top 10 ruleset — p/owasp-top-ten)
```
Exit: 0 — no security findings in Rust/Python/Go/JS source files.
(GitHub Actions workflow variables noted, informational only)
```

### Trivy v0.69.3
- **Main project `Cargo.lock`**: 0 vulnerabilities
- Worktrees (`vigilant-ishizaka-5f7a25`, `youthful-cartwright-40fac1`): 1 HIGH each
  - `rustls-webpki` 0.103.12 → GHSA-82j2-j2ch-gfr8 (DoS via malformed CRL BIT STRING)
  - **Not applicable**: main project already uses rustls-webpki 0.103.13 (fixed version)

### TruffleHog v3.94.2
```
chunks: 12292 | bytes: 8,727,532
verified_secrets: 0 | unverified_secrets: 0
scan_duration: 2.011748s
```
Clean — no secrets detected.

### OSV-Scanner v2.3.5
- Main `Cargo.lock` scan: **No issues found** (203 packages, 0 vulnerabilities)
- Recursive scan found 13x GHSA-82j2-j2ch-gfr8 — all from stale worktree `Cargo.lock` files, not the main project

### Config-Audit (config-audit.py)
| Severity | Finding | Assessment |
|----------|---------|------------|
| MEDIUM | `CLAUDE.md` — "instruction to skip verification" | False positive: anti-rabbit-hole workflow guidance, not a security bypass |
| MEDIUM | `CLAUDE.md` — "trust-all instruction" | False positive: "trust snapshot tests" refers to test strategy, not security trust |
| LOW | `settings.json` hooks | Expected: standard Claude Code hook configuration |
| LOW | Plugin hooks (pordee, codex, addy, claude-plugins-official) | Expected: standard plugin hooks |

## Upstream Changes Merged (6 commits)
| Commit | Change |
|--------|--------|
| `63a76de` | Merge PR #1645 from KuaaMU/master |
| `9574007` | fix(aws): preserve values in JSON output for unsupported subcommands |
| `35273c2` | fix(curl): passthrough binary downloads to prevent UTF-8 corruption |
| `a2a63e1` | fix(curl): passthrough binary downloads to prevent UTF-8 corruption |
| `ad2bfd3` | fix(aws): preserve values in JSON output for unsupported subcommands |
| `6b30fdd` | fix(filters): remove max_lines cap from helm filter that truncates template output |

Files changed: `src/cmds/cloud/aws_cmd.rs`, `src/cmds/cloud/curl_cmd.rs`, `src/filters/helm.toml`, `tests/fixtures/aws_backup_describe_global_settings.json`
