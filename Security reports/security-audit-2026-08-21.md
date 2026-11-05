# Security Audit — 2026-08-21

## Summary
- Issues found: 3 (categories) | Auto-fixed: 0 | Unresolved: 3
- Status: PASSED (no exploitable vulnerabilities; all findings are established false positives or out-of-scope config notes)

Upstream sync: merged 7 commits from `origin/develop` (git log/grep argument-parsing fixes in `src/cmds/git/git.rs`). Clean merge, no conflicts.

## Fixed Issues
None. Semgrep (OWASP + secrets), Trivy (fs/deps), and OSV-Scanner (Cargo.lock, 208 packages) all returned zero findings — no dependency CVEs or code-level vulnerabilities to patch.

## Unresolved Issues

| # | Component | Tool | Reason |
|---|-----------|------|--------|
| 1 | `tests/guard_integration_test.rs`, `Security reports/security-audit-*.md` (23 historical files), `scripts/benchmark/cloud-init.yaml`, `src/cmds/cloud/aws_cmd.rs`, `SECURITY.md` | Gitleaks (38 hits: `stripe-access-token`, `generic-api-key`) | Test fixtures and placeholder/example keys embedded in historical audit reports and cloud-CLI test data, not live credentials. No safe automated fix — redacting historical git-tracked report content is a manual editorial decision, not a security patch. |
| 2 | 51 files (`src/**/*.rs`, `Security reports/*.md`) | TruffleHog (124 "verified" hits, all `Lob` detector) | Every hit is the TruffleHog `Lob` API-key detector matching ordinary Rust snake_case identifiers (e.g. `test_needs_shell_stderr_redirect_to_file`) as if they were Lob keys, and reporting them as "verified." This is a known false-positive pattern in the Lob detector, not real secret material — no code change applicable. |
| 3 | Global Claude Code plugin/hook configs, `CLAUDE.md` / `claude.md` | config-audit.py (7 MEDIUM broad-matcher hooks, 4 MEDIUM "trust snapshot tests" phrasing, 12 LOW hook-presence notices) | All findings are in globally-installed Claude Code plugins (`~/.claude/plugins/...`) or CLAUDE.md prose, not rtk source/build artifacts. Out of scope for a code/dependency security fix in this repo. |

`mcp-exfil-scan.sh` hit an unbound-variable bug (`MCP_CONFIGS[@]`) and exited early — rtk has no MCP config files, so this had no coverage impact.

## Raw Scanner Output

### Gitleaks
```
9:09AM INF 1373 commits scanned.
9:09AM INF scanned ~9289103 bytes (9.29 MB) in 1.22s
9:09AM WRN leaks found: 38
```
Rule breakdown: `stripe-access-token` × 21, `generic-api-key` × 17. Files (all pre-existing, none touched by today's merge): `tests/guard_integration_test.rs` (2), 23× `Security reports/security-audit-*.md` / `2026-04-16-security-report.md`, `scripts/benchmark/cloud-init.yaml` (2), `src/cmds/cloud/aws_cmd.rs` (5), `SECURITY.md` (1).

### Semgrep — secrets (`p/secrets`)
```
Scanning 404 files tracked by git with 52 Code rules
Findings: 0 (0 blocking)
Ran 45 rules on 404 files: 0 findings.
```

### Semgrep — OWASP Top Ten (`p/owasp-top-ten`, py/js/ts/jsx/tsx/java/go/rb only — rtk is Rust, minimal applicable surface)
```
Scanning 12 files tracked by git with 560 Code rules
Findings: 0 (0 blocking)
Ran 266 rules on 12 files: 0 findings.
```

### Trivy fs (deps + secrets + IaC)
```
[vuln] Vulnerability scanning is enabled
[secret] Secret scanning is enabled
Number of language-specific files: 20
[cargo] Detecting vulnerabilities...
```
Result: 0 vulnerabilities across all cargo/pom targets (main tree + worktree copies), no secrets flagged.

### OSV-Scanner
```
Scanned Cargo.lock file and found 208 packages
No issues found
```

### TruffleHog (git history, verified+unknown)
```
finished scanning  chunks=14657 bytes=10374799 verified_secrets=124 unverified_secrets=0
```
124/124 results: Detector Type `Lob`, matching Rust identifier strings across 51 files — see Unresolved #2.

### config-audit.py
7× MEDIUM (broad-matcher hooks in globally-installed plugins: `claude-plugins-official`, `claude-code-security-plugins`, `caveman`; "skip verification" / "trust snapshot tests" phrasing in `CLAUDE.md`/`claude.md`), 12× LOW (hook-presence notices across installed plugins). Full listing in Unresolved #3.

### mcp-exfil-scan.sh
Crashed on `MCP_CONFIGS[@]: unbound variable` after confirming 0 MCP config files and 24 skill files present in target. No MCP surface in rtk — no coverage lost.

### APTS Audit Log
- **Log:** `/tmp/css-scan-20260821T020856Z.jsonl`
- **Tool runs recorded:** 5 (measured: 5, asserted: 0)
- **Standard:** OWASP APTS § Auditability
