# Security Audit — 2026-08-20

## Summary
- Issues found: 0 real | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED
- 5 upstream commits merged (tee.rs collision-hash fix, benchmark.sh harness-wipe fix). No new vulnerabilities introduced. All scanner hits reviewed below are confirmed false positives.

## Fixed Issues
None — no real vulnerabilities found.

## Unresolved Issues
None.

## False Positives Reviewed (no action needed)

| # | Tool | Finding | Why it's not real |
|---|------|---------|--------------------|
| 1 | Gitleaks | 38 "leaks" (stripe/generic API key patterns) | All are placeholder strings (`sk_live_1234567890ab...`, `FAKE...`) in `SECURITY.md`, historical `Security reports/*.md`, `scripts/benchmark/cloud-init.yaml` example config, `src/cmds/cloud/aws_cmd.rs` (secret-redaction test fixtures), and `tests/guard_integration_test.rs` (explicitly named `FAKE...`) |
| 2 | Bandit | 9 Low-severity `subprocess_without_shell_equals_true` | All use list-form args (no `shell=True`), not exploitable |
| 3 | TruffleHog | 125 "✅ verified" secrets, all `Detector Type: Lob` | Lob (mail/print API) detector matching `test_...`-shaped strings across historic commits (e.g. test function names). rtk has zero Lob API usage; Lob's sandbox verifies any `test_`-prefixed string as valid — known false-positive class |
| 4 | config-audit.py | 2 MEDIUM "suspicious instruction" hits in `claude.md` | Project's own "Avoiding Rabbit Holes" section (legitimate dev guidance to avoid over-verification), not a prompt-injection attempt |
| 5 | config-audit.py | Multiple LOW "hooks configuration found" | All from known/installed marketplace plugins (caveman, engram, impeccable, etc.), not attacker-controlled |

## Clean Scans
- **Trivy** (fs, deps + secrets + IaC): 0 vulnerabilities across all `Cargo.lock`/`pom.xml` targets
- **OSV-Scanner**: 208 Cargo packages scanned, 0 issues
- **Semgrep** (OWASP Top 10, secrets, python configs): 0 findings across all applicable rule sets
- **mcp-exfil-scan**: 0 MCP configs present in rtk itself (script hit an unrelated `unbound variable` bug on line 151/197 of the bundled script — noted as a tool defect, not a finding; scan target has no MCP surface to exfiltrate from)

## Coverage Gaps
- CodeQL skipped (no `.github/workflows/codeql.yml` configured for this repo)
- mcp-scan and skillspector LLM-mode skipped (opt-in tools requiring live user consent; no user present in this autonomous run)
- mcp-exfil-scan aborted early on an unbound-variable bug in the bundled script (`scripts/mcp-exfil-scan.sh:151,197`) — worth a bug report upstream to the security-scanner plugin, but immaterial here since 0 MCP configs exist in this target
- Business-logic / runtime behavior not covered by static tools

## Raw Scanner Output

### Gitleaks
```
9:09AM INF 1362 commits scanned.
9:09AM INF scanned ~9225895 bytes (9.23 MB) in 1.33s
9:09AM WRN leaks found: 38
```
(38 findings — see False Positives table above for full file:line breakdown)

### Bandit
```
Total issues (by severity):
    Undefined: 0
    Low: 9
    Medium: 0
    High: 0
Total issues (by confidence):
    Undefined: 0
    Low: 0
    Medium: 0
    High: 9
Files skipped (0)
```
All 9 are B603 `subprocess_without_shell_equals_true` in `scripts/benchmark-sessions/lib/runner.py` (list-arg `subprocess.run`, safe).

### Trivy
```
Report Summary: all targets (Cargo.lock x4 worktrees, pom.xml fixtures) — Vulnerabilities: 0, Secrets: -
```

### OSV-Scanner
```
End status: 743 dirs visited, 6824 inodes visited
No issues found
```

### TruffleHog
```
{"chunks": 14458, "bytes": 10307844, "verified_secrets": 125, "unverified_secrets": 0}
```
All 125 `Detector Type: Lob` — see False Positives table.

### Semgrep
```
OWASP Top Ten:   Ran 266 rules on 12 files: 0 findings.
Secrets:         Ran 45 rules on 403 files: 0 findings.
Python configs:  Ran 151 rules on 2 files: 0 findings.
```

### config-audit.py (Claude config audit)
2 MEDIUM (claude.md dev-guidance false positives), remainder LOW (known plugin hooks). Full output in `/tmp/rtk-secscan/config-audit.txt`.

### mcp-exfil-scan
Aborted on `MCP_CONFIGS[@]: unbound variable` (script bug) after confirming `MCP configs found: 0`, `Skill files found: 24` for this target.

## Build Notes
This run also performed STEP 5 (build/install) and STEP 6 (commit/push) per the daily-sync task — see the run's top-level summary output for binary version and push status.
