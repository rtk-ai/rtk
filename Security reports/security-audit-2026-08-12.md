# Security Audit — 2026-08-12

## Summary
- Issues found: 3 categories (161 raw findings) | Auto-fixed: 0 | Unresolved: 0 (all triaged as false positive / accepted-risk, no code change required)
- Status: PASSED

## Fixed Issues
None — no true vulnerabilities requiring a code or dependency change were found.

## Triaged Findings (no fix required)

| # | Component | Tool | Finding | Disposition |
|---|-----------|------|---------|--------------|
| 1 | `Security reports/security-audit-*.md` (26 hits), `SECURITY.md`, `scripts/benchmark/cloud-init.yaml` | Gitleaks | `stripe-access-token` / `generic-api-key` pattern matches | **False positive.** These are prior audit reports and a cloud-init fixture containing example/placeholder strings that happen to match Stripe/generic key regexes; not live credentials. |
| 2 | `src/cmds/cloud/aws_cmd.rs:1878-2035` | Gitleaks | `generic-api-key` pattern matches | **False positive.** Matches are inside `#[cfg(test)]` unit tests — synthetic AWS CloudWatch Logs pagination tokens (`nextForwardToken`) used as filter-output fixtures, not real keys. |
| 3 | Repo-wide (116 hits, `src/cmd/hook/mod.rs`, `src/discover/registry.rs`, `src/cmds/jvm/gradlew_cmd.rs`) | TruffleHog | "Verified" `Lob` API key detector hits | **False positive.** TruffleHog's Lob detector pattern-matches Rust test function names (e.g. `test_transparent_sinks_contains_expected`) as if they were Lob API keys and "verifies" against Lob's API, which apparently accepts these strings. No real secret present. |
| 4 | `scripts/benchmark-sessions/lib/runner.py:5,28` | Bandit | B404/B603/B607 — subprocess usage / partial executable path | **Accepted risk, no fix needed.** `subprocess.run(["tar", "czf", ...])` uses a hardcoded argument list with no user-controlled input reaching the shell; this is a local benchmarking utility, not production/user-facing code. |

## Clean Scanners
- **Semgrep** (OWASP Top Ten, Python, secrets configs): 0 findings across all three passes.
- **Trivy** (filesystem — deps + secrets, incl. all `Cargo.lock` and `pom.xml` fixtures): 0 vulnerabilities, 0 secrets.
- **OSV-Scanner** (source scan, `Cargo.lock` — 202 packages, plus Maven fixtures): No issues found.
- **CodeQL**: Skipped — no `.github/workflows/codeql.yml` present on the `fork`/`origin` remotes.
- **mcps-audit / skill-audit**: Skipped — no `*.skill`, `SKILL.md`, or `mcp*.json` files inside the target path.
- **config-audit** (Claude config/hooks): Only global `~/.claude` plugin ecosystem hooks flagged (MEDIUM, broad SessionStart/UserPromptSubmit matchers — expected for installed plugins) and two MEDIUM notes on `CLAUDE.md`'s "avoid rabbit holes" guidance being phraseable as a "skip verification" instruction. Not repo vulnerabilities; no action taken.
- **mcp-exfil-scan**: Bundled script hit an `unbound variable` bug (`MCP_CONFIGS[@]`) when zero MCP config files are present and exited early — see Coverage Gaps. Target repo has 0 MCP configs, so exposure is believed nil, but the scanner itself did not complete cleanly.

## Unresolved Issues
None.

## Raw Scanner Output

### Pre-flight
```
OK  bandit bandit 1.9.4
OK  semgrep (pipx build, $HOME/.local/bin/semgrep)
OK  trivy Version: 0.72.0
OK  trufflehog trufflehog 3.95.9
OK  gitleaks gitleaks version 8.30.1
OK  osv-scanner osv-scanner version: 2.4.0
OK  gh
OK  npx
OK  jq
```

### Gitleaks
```
1323 commits scanned, ~9.07 MB scanned in 975ms
leaks found: 36 (all triaged as false positive — see table above)
```

### Bandit
```
Total issues (by severity): Low: 9, Medium: 0, High: 0
Total issues (by confidence): High: 9
All 9 in scripts/benchmark-sessions/lib/runner.py (B404, B603, B607 — subprocess usage)
```

### Semgrep
```
p/python:        151 rules × 2 files  → 0 findings
p/secrets:        45 rules × 399 files → 0 findings
p/owasp-top-ten: 266 rules × 12 files  → 0 findings
```

### Trivy
```
trivy fs <target>
20 language-specific manifests scanned (Cargo.lock ×4 worktree copies, pom.xml fixtures ×20)
All targets: 0 vulnerabilities, 0 secrets
```

### TruffleHog
```
git file://<target> --no-update
chunks: 14137, bytes: 10126582
verified_secrets: 116 (Lob detector false positives against test function names)
unverified_secrets: 0
```

### OSV-Scanner
```
osv-scanner scan source -r <target>
Cargo.lock: 202 packages scanned (×4 copies across worktrees)
No issues found
```

### config-audit (Claude config/hooks/CLAUDE.md)
```
12× MEDIUM — broad-matcher SessionStart/UserPromptSubmit hooks in globally
              installed Claude Code plugins (claude-plugins-official, caveman,
              pordee, claude-code-security-plugins) — expected plugin behavior,
              not repo-specific.
2× MEDIUM  — CLAUDE.md / claude.md "Avoiding Rabbit Holes" section phrased in a
              way the auditor's heuristic flags as "instruction to skip
              verification" / "trust-all instruction". Intent (per the file) is
              scope discipline, not suppressing genuine security checks.
10× LOW    — hooks configuration present (informational only).
```

### mcp-exfil-scan
```
MCP configs found: 0
Skill files found: 24
Script errored: line 151/197 "MCP_CONFIGS[@]: unbound variable" (bash `set -u`
  incompatibility when the MCP_CONFIGS array is empty) — scan did not complete.
No MCP config files exist in this repo, so likely low actual exposure, but
flagged as a Coverage Gap since the tool itself failed rather than returning
a clean 0-finding result.
```

### APTS Audit Log
- **Log:** `/tmp/css-scan-20260812T021649Z.jsonl`
- **Tool runs recorded:** 11 (measured: 11, asserted: 0)
- **Standard:** OWASP APTS § Auditability
