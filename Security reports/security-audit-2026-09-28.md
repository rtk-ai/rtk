# Security Audit — 2026-09-28

## Summary
- Issues found: 0 real vulnerabilities (49 scanner hits triaged, all false-positive/benign) | Auto-fixed: 0 | Unresolved: 1 (tool skipped)
- Status: PASSED

## Scope Record
```
Scan target: /Users/mp3wizard/Public/Claude Proxy/rtk
Git HEAD:    25cbeebc (post-merge, chore: sync upstream 2026-09-28)
Include:     all supported (Rust source, Cargo.toml, Cargo.lock, plus repo-wide secret/config scan)
Exclude:     target/, .git/, .claude/worktrees/* (nested worktree copies, out of scope)
```

## Fixed Issues
_None — no exploitable vulnerability was found; nothing required a code or dependency change._

## Reviewed / False-Positive Findings
| # | Component | Tool | Finding | Disposition |
|---|-----------|------|---------|-------------|
| 1 | `tests/guard_integration_test.rs:166,184` | Gitleaks | `stripe-access-token` pattern | False positive — literal `sk_live_FAKE...` string used as a git-log test fixture |
| 2 | `src/cmds/cloud/aws_cmd.rs:1878-2035` | Gitleaks | `generic-api-key` pattern | False positive — synthetic pagination/token strings in `#[cfg(test)]` fixtures |
| 3 | `Security reports/*.md` (28 files, historical) | Gitleaks | `stripe-access-token` / `generic-api-key` | False positive — prior audit reports quoting the same test fixtures verbatim |
| 4 | `scripts/benchmark/cloud-init.yaml:282,613` | Gitleaks | `generic-api-key` | False positive — placeholder key strings in a benchmark cloud-init template |
| 5 | `SECURITY.md:151` | Gitleaks | `stripe-access-token` | False positive — documentation example |
| 6 | `scripts/benchmark-sessions/lib/runner.py:28` | Bandit | B603 subprocess without `shell=True` (9 occurrences) | Low/informational — all calls use list-form argv (`["tar", "czf", ...]`), no shell interpolation, no untrusted input |
| 7 | `claude.md` (repo root) | config-audit | "skip verification" / "trust-all" phrasing | False positive — project's own "Avoiding Rabbit Holes" policy text, not an injected instruction |

## Unresolved Issues
| # | Tool | Reason |
|---|------|--------|
| 1 | mcp-exfil-scan | Bundled script `mcp-exfil-scan.sh` failed the SHA256SUMS integrity check in this plugin install (`claude-code-security-plugins` v1.8.0). Per the skill's manipulation-resistance gate, a checksum mismatch means "do not run." Not rtk-repo-related; flag to reinstall/verify the security-scanner plugin cache. No skill/MCP artifacts were found in the rtk repo scope anyway (`find` for `*.skill`/`SKILL.md`/`mcp*.json` returned nothing), so the practical impact this run is zero. |

## Clean Scanner Results
- **Semgrep** (secrets + OWASP Top Ten configs): 0 findings across 398 tracked files.
- **Trivy** (fs scan, deps + secrets): 0 vulnerabilities, 0 secrets (`Cargo.lock`, test-fixture `pom.xml` files).
- **TruffleHog** (git history, full repo): 0 verified, 0 unverified secrets across 1893 commits / 20,630 chunks.
- **OSV-Scanner** (SCA, `Cargo.lock` + fixture `pom.xml`): "No issues found."
- **Gitleaks**: 38 raw hits, all triaged above as false positives (pre-existing test fixtures / historical report files, not live secrets).

## Raw Scanner Output
Full untruncated output for each tool run is preserved in the APTS audit workspace for this session (gitleaks.sarif, gitleaks.txt, bandit.txt, semgrep_secrets.txt, semgrep_owasp.txt, trivy.txt, trufflehog.txt, osv.txt, config-audit.txt). Key excerpts are quoted inline above; nothing was truncated or redacted beyond the standard secret-value redaction policy (no live secrets were found, so no redaction was needed).

### APTS Audit Log
- Log: `/tmp/css-scan-20260928T021631Z.jsonl`
- Tool runs recorded: 10 (measured: 10, asserted: 0)
- Standard: OWASP APTS § Auditability
