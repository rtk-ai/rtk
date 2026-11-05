# Security Audit — 2026-08-27

## Summary
- Issues found: 0 real vulnerabilities | Auto-fixed: 0 | Unresolved: 0
- Scanner alerts triaged: 168 (38 gitleaks + 127 trufflehog + 3 skill-audit) — all confirmed false positives
- Status: PASSED

## Fixed Issues
None — no real vulnerabilities found in Rust source, Cargo.toml, or Cargo.lock.

## Unresolved Issues
None.

## False Positives Reviewed (not counted as issues)

| # | Tool | Finding | Location | Reason |
|---|------|---------|----------|--------|
| 1 | Gitleaks | 38 "secrets" (stripe-access-token, generic-api-key) | `tests/guard_integration_test.rs`, `Security reports/*.md`, `SECURITY.md`, `src/cmds/cloud/aws_cmd.rs`, `scripts/benchmark/cloud-init.yaml` | All are test fixtures, documentation placeholder keys (`msk_live_1234567890abcdef`, `secret-api-key-12345`), or mock AWS CloudWatch pagination tokens. No live credentials. |
| 2 | TruffleHog | 127 "verified" secrets, Detector Type: Lob | `src/learn/detector.rs` | Known Lob-detector false-positive pattern — matched test function name strings (e.g. `test_deduplicate_corrections_merges_same`) against Lob's loose key format, not actual Lob API keys. |
| 3 | skill-audit.sh | CRITICAL — `.claude/skills/security-guardian/SKILL.md` | risk 100/100 | Flags its own documentation examples of attack strings (`; rm -rf /`, `cat /etc/passwd`) and a *blocklist* array (`["rm","dd","mkfs"]`) as if they were executed dangerous code. They are illustrative examples inside a security-review skill, never executed. |
| 4 | skill-audit.sh | CRITICAL — `.claude/skills/ship/SKILL.md` | risk 90/100 | Flags Bash+Write+Edit tool access (needed for its release workflow) plus a regex match on the literal string `.env` (used to warn against committing secrets, not to read one) and a trusted `github.com/rtk-ai/rtk` release URL. |
| 5 | skill-audit.sh | HIGH — `.claude/skills/performance/SKILL.md` | risk 65/100 | Flags `sudo dtrace` inside a documentation table of macOS profiling techniques as "privilege escalation." It is reference material, not code the skill runs. |

## Clean Results

| Tool | Result |
|------|--------|
| Trivy (fs scan, deps + secrets) | 0 vulnerabilities across all `Cargo.lock` / `pom.xml` targets |
| OSV-Scanner (208 Cargo packages + fixture poms) | No issues found |
| Semgrep `p/owasp-top-ten` (Rust) | 0 findings (121 files, 6 applicable rules) |
| Semgrep `p/secrets` | 0 findings (405 files, 45 rules) |
| Bandit | Skipped — no `.py` files in project scope |
| config-audit.py (Claude config/hooks audit) | Only LOW-severity "hooks configuration found" entries, all inside globally-installed plugin caches (not this project) |
| CodeQL | Skipped — no CodeQL workflow in `.github/workflows/` |
| mcps-audit | Skipped — no MCP manifest files in project scope |
| mcp-exfil-scan.sh | **Crashed** on `MCP_CONFIGS[@]: unbound variable` (bundled-script bug under `set -u` when zero MCP configs exist) — see Coverage Gaps |

## Coverage Gaps
- `mcp-exfil-scan.sh` (bundled script, security-scanner plugin v1.8.0) has a `set -u` bug: it references `MCP_CONFIGS[@]` before the array is ever populated, aborting at step 1/6 whenever a project has zero MCP config files (as here). Exfiltration-chain analysis for this project is therefore incomplete — flagging as a scanner defect to fix upstream, not a project vulnerability.
- mcp-scan and skillspector LLM-mode were skipped (opt-in, require user consent for external data transmission; no user present in this autonomous run).
- Business logic / IDOR / runtime behavior not covered by static tools.

## Dependency Sync
Merged 20 new commits from `origin/develop` (upstream `rtk-ai/rtk`) — notably a rewrite of `src/cmds/system/find_cmd.rs` (883 lines changed) and Pi hook tweaks. Merge was clean, no conflicts.

## Build
`cargo install --path . --force` — see final run summary for result. Cargo.toml version at merge: see build step output.

## Raw Scanner Output

### Gitleaks
```
9:09AM INF 1405 commits scanned.
9:09AM INF scanned ~9532802 bytes (9.53 MB) in 1.13s
9:09AM WRN leaks found: 38
```
(38 findings detailed and triaged above — full verbose output available via `gitleaks detect --source . -v`)

### TruffleHog
```
2026-08-27T09:09:45+07:00 info-0 trufflehog finished scanning {"chunks": 15012, "bytes": 10645883, "verified_secrets": 127, "unverified_secrets": 0, "scan_duration": "1.779279833s", "trufflehog_version": "3.95.9"}
```
All 127 = Detector Type: Lob (false positives, see above).

### Trivy
```
Report Summary — all cargo/pom targets: 0 vulnerabilities, 0 secrets across the full repo tree.
```

### OSV-Scanner
```
Scanned Cargo.lock file and found 208 packages
No issues found
```

### Semgrep (OWASP + secrets)
```
Ran 6 rules on 121 files: 0 findings.  (p/owasp-top-ten, *.rs)
Ran 45 rules on 405 files: 0 findings. (p/secrets, full tree)
```

### config-audit.py
Only LOW-severity hook-presence notices, all under globally installed plugin caches — no project-specific findings.

### skill-audit.sh
12 unique project `SKILL.md` files scanned (`.claude/skills/*`): 5 LOW, 4 MEDIUM, 1 HIGH, 2 CRITICAL by raw heuristic score — all 3 non-LOW/MEDIUM verdicts reviewed and dismissed as false positives (see table above).

### APTS Audit Log
- **Log:** `/tmp/css-scan-20260827T020920Z.jsonl`
- **Tool runs recorded:** 6 (measured: 6, asserted: 0)
- **Standard:** OWASP APTS § Auditability
